# mDNS 多网卡测试失败与缓存归属修复

日期：2026-10-22。本机 macOS arm64，源码基线 `53ed907`，版本 `0.1.7-beta.1`。本次继续追查此前内存修复中留下的上游测试失败。

后续正式版 review 补齐了已知答案接口隔离、测试端口初始化和上游集成夹具，最新验证见 [0.1.7 发布前 review](release-0.1.7-review-2026-10-22.md)。下文保留本轮本机替换时的验证快照。

## 原失败的具体原因

本机以太网 `en0` 和 Wi-Fi `en1` 位于同一网段，同一服务可通过两张网卡收到。

- `test_hostname_resolution_address_removed` 从接口列表选取 `en0`，构造包含该网卡信息的 `ScopedIp`，但 server/client 实际启用了全部网卡。失败日志中，预期与实际 IPv4 相同，预期 scope 为 `en0`，第一条事件的 scope 为 `en1`。测试把报文到达顺序当成了保证。
- `test_interface_flip` 同样启用了全部网卡，却要求关闭 `en0` 后服务消失。失败日志显示服务最初经 `en1` 解析，后来同时经 `en0`、`en1` 解析，关闭 `en0` 后仍能经 `en1` 解析，因此不应收到完全移除事件。
- 完整回归中的 `test_refresh_ptr` 也假定只有一份 PTR。按接口正确保存记录后，两条链路产生两份刷新记录，原来的固定计数 1 不再适用于多网卡测试环境。

这三项测试改用一个 loopback IPv4 接口和动态私有 UDP 端口，仍然严格验证地址与接口、TTL 移除、服务移除/恢复，以及单接口刷新计数。没有改为忽略测试或放松为“任意结果”。地址过期和接口恢复的等待使用总截止时间，测试结束或断言失败时清理 daemon；不修改系统网卡状态。

接口恢复测试还需要等待发布端完成两次启动公告。原先立即移除接口可能与第二次公告相撞，恢复查询在一秒内受到正常的 multicast rate limit 抑制，随后四秒的 browse 退避恰好超过测试截止时间。已用延迟响应发送前后日志确认，改为等待公告事件完成，而非任意增加 sleep 或放宽结果。

隔离测试也发现自定义端口查询仍用常量 5353 判断 legacy unicast，已改为当前 daemon 配置的端口。默认 5353 下的协议行为保持一致，原有 legacy unicast 回归通过。

## 同时确认的真实缓存缺陷

不能仅把这次失败归为环境问题。进一步用两张模拟网卡注入相同 PTR/SRV/TXT 后，确认旧版缓存只保留第一张网卡的归属，第二张仅刷新 TTL。移除第一张时，会错误移除另一张仍能发现的服务。换一个移除顺序又可能正常，导致行为依赖报文顺序。

修复如下：

- 缓存记录按接收接口匹配和更新，同接口刷新复用记录，不同接口独立维护生命周期。
- cache-flush、过期响应清理仅影响接收接口。一个接口的 PTR 过期时，若其他接口仍有有效记录，不发送完全移除事件。
- 移除最后一个接口后才完全移除服务；剩余接口的地址变化继续通过解析事件上报。
- 对合并后的发现通知和后续查询去重，避免每份接口记录产生重复事件。
- 保留上次的无消费者缓存过滤、独立过期回收及稀疏 HashMap 容量回收。

新增四项缓存回归覆盖两个移除顺序、两个 TTL 顺序、跨接口 cache-flush 隔离，以及每张网卡重复刷新 1,000 次仍仅保留两份记录。它们在未修改的上游基线上全部失败，在修复版全部通过。另有 daemon 回归覆盖过期响应只移除一条链路、剩余地址更新、最后一条链路移除及缓存查询通知去重。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| `cargo test --manifest-path vendor/mdns-sd/Cargo.toml --locked --lib -- --test-threads=1` | 82 项通过，0 失败，0 忽略 |
| 原失败的两项测试及刷新测试，每项分别连续运行 5 次 | 15 次全部通过 |
| `cargo test --locked -p removent-net -p removent-daemon -p removent-host -p removent-client -- --test-threads=1` | 257 项通过，3 项原有忽略 |
| 上述四个 Removent 包全部 targets 的 `clippy -D warnings` | 通过；原有 `block 0.1.6` future-incompatibility 提示仍在 |
| workspace 格式、修改过的 vendor 文件格式、布局、diff 空白 | 通过 |
| 私有端口 UDP 内存探针，5 轮共 20,000 条记录 | 每轮过期后缓存归零、定时器回到 1；仅发布端始终不缓存 |

探针每轮过期后的 footprint 为 4,240、4,624、4,688、4,848、4,784 KiB，仍处于约 4–5 MiB 的平台。CI 已从仅运行内存测试扩展为运行完整 mDNS 库测试及 UDP 内存探针；这里记录的是本地结果，未运行远端 CI。

额外对 vendored 上游库直接执行 `clippy --lib --tests -- -D warnings`，在库阶段遇到原有风格 lint：59 项 `useless_borrows_in_formatting`、7 项 `for_kv_map`。用未修改上游对照后，诊断内容与数量完全相同，未新增运行时代码 lint。没有批量修改上游无关风格，也未在仓库添加 lint 屏蔽；这不属于现有 workspace CI 的检查范围。

## 内存观察

重新安装前，对上一轮内存修复版的同一进程复测：PID `68108`，从 2026-10-21 22:13:53 运行至 2026-10-22 02:19:48，持续约 4 小时 6 分钟。physical footprint 及其峰值均为 4,689 KiB（约 4.58 MiB），RSS 为 14,256 KiB。没有把 RSS 与 footprint 混用，也没有把该观察当成原现场约 46 小时的等时对照。

本次验证未进行其他实体设备的远程连接、真实系统网卡断开或约 46 小时连续观察。源码尚未提交、推送或发布。

## 本机替换结果

已重新签名构建并安装到 `/Applications/Removent.app`，版本 `0.1.7-beta.1`，revision `53ed907-local`。

- source SHA-256：`68c86dc64ae86500350945f89dd1fffdc75e7f524b21721f6d63fac0a7992dea`；安装后源码指纹验证通过。
- daemon SHA-256：`a3e951dbf686eb97be79c73fdaaca7ba9d91791873fe71d376f8fefaaa7167dc`；主程序、CLI、daemon、托盘的已安装文件与构建产物哈希一致。
- 新 daemon PID `61745`，从 bundle 内 `RemoventHost.app` 启动，启动约 33 秒时 footprint 为 4,497 KiB（约 4.39 MiB）。
- `running=true`、`host_ready=true`、`tray_connected=true`，屏幕录制和辅助功能授权均为 true，`sessions=[]`。
- 登录启动保持开启，服务 `managed=true`、`reachable=true`。
