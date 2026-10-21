# daemon 空闲内存持续增长修复

日期：2026-10-21。环境：本机 macOS arm64。源码基线：`53ed907`，版本 `0.1.7-beta.1`。

2026-10-22 后续：本文记录的两项上游测试失败已继续定位并修复，完整库测试现为 82 项通过；同时修复了多网卡缓存归属缺陷，详见 [mDNS 多网卡回归](mdns-multi-interface-2026-10-22.md)。下文保留首次验证结果。

## 问题与依据

原本安装的 `0.1.6` daemon 在空闲约 45 小时 44 分钟、没有远程会话时，`vmmap` physical footprint 为 719.8 MiB，存活堆分配为 666.4 MiB，约 620 万次分配。RSS 约 132 MiB，单看 RSS 会漏掉压缩或换出的内存。

1,000 次状态 IPC 请求仅改变约 0.1 MiB 的 footprint。重建共享 runner 后，存活堆回落到 555 KiB；分配器仍保留部分峰值内存。重启同一个旧版二进制后，footprint 为 4,417 KiB。这些测量将排查范围缩小到共享 runner 生命周期内的状态。

`mdns-sd 0.21.5` 会接收不含 PTR 的响应，即使当前只发布服务、没有发现或主机名解析消费者，也会缓存 SRV/TXT。原过期清理通过 PTR 遍历关联记录，漏掉没有 PTR 的 SRV/TXT，以及 NSEC。隔离 UDP 复现中，2,000 组 TTL 为 1 秒的 SRV/TXT 在发送结束 4 秒后仍保留 4,000 条。

因此已证实该缓存存在持续保留缺陷，且与现场生命周期测量一致。没有捕获原进程的历史报文，不能把原先 719.8 MiB 的每一项分配都归因于特定 DNS 记录。

## 实现

- 通过 workspace Cargo patch 引入保留原许可证的 `vendor/mdns-sd`，固定版本以防更新绕过补丁。daemon、desktop 和移动端使用同一修复。
- 仅发布服务且未明确启用 unsolicited 缓存时，不保存收到的响应。名称冲突检测先于此判断，仍然执行。
- 独立清理 PTR、SRV、TXT、NSEC 中的过期记录，删除空键和失效 subtype 映射；保留原先的服务移除及地址移除通知。
- 稀疏 HashMap 超过 64 容量且使用率低于四分之一时收缩，避免一直保留历史峰值容量，同时避免频繁扩缩容。
- 新增 10 项确定性内存回归测试和独立端口的真实 UDP 压力探针，并将两者加入 CI。补丁来源及移除条件见 `vendor/mdns-sd/REMOVENT.md`。

## 本地验证

| 检查 | 结果 |
| --- | --- |
| `cargo test --manifest-path vendor/mdns-sd/Cargo.toml --locked --lib memory_tests -- --test-threads=1` | 10 项通过 |
| `cargo test --locked -p removent-net -p removent-daemon -p removent-host -- --test-threads=1` | 121 项通过，1 项忽略 |
| `cargo test --locked -p removent-client -- --test-threads=1` | 136 项通过，2 项忽略 |
| `cargo clippy --locked -p removent-net -p removent-daemon -p removent-host --all-targets -- -D warnings` | 通过；Cargo 仍提示已有 `block 0.1.6` 的 future-incompatibility |
| workspace 格式、修改过的 vendor Rust 格式、diff 空白检查 | 通过 |
| `bash scripts/check.sh` | 通过，包含布局、94 项 Python 测试（1 项跳过）及 Shell 语法 |
| 签名构建、本地安装、安装后源码指纹与二进制哈希校验 | 通过 |

将最初 9 项新回归测试放入未修改的上游源码，7 项失败、2 项通过；同样的测试在修复版通过。后来补充的第 10 项测试覆盖大量过期后仍保留少量有效记录的容量回收。

上游完整库测试曾得到 74 项通过、2 项失败（当时尚未加入第 10 项测试）。`test_hostname_resolution_address_removed` 与 `test_interface_flip` 在本机多网卡环境失败；未修改的上游基线也复现相同失败，后者连续 3 次失败。因此不宣称上游完整测试全绿，也未删除或忽略这些测试。Removent 自身的发现、兼容发现、更新和 goodbye 相关测试通过。

### 真实 UDP 循环压力

命令：`cargo run --manifest-path vendor/mdns-sd/Cargo.toml --locked --example memory_probe`。

仅使用 loopback 上动态分配的私有端口，不向生产 mDNS 5353 端口注入数据。每轮发送 2,000 组 SRV/TXT，共 5 轮、20,000 条记录，每条 TTL 为 1 秒。

- 仅发布端的 SRV/TXT 缓存始终为 0。
- 显式启用缓存的一端每轮确认收到记录，过期后 SRV/TXT 均为 0。
- 两端每轮过期后均回到 1 个后台定时器，没有随轮次累积。
- 探针进程每轮过期后 footprint 为 4,176、4,480、4,672、4,848、4,624 KiB，进入约 4–5 MiB 的平台，未出现原先的记录累积。

### 已安装进程

已替换 `/Applications/Removent.app` 并启动，版本 `0.1.7-beta.1`，revision `53ed907-local`。

- source SHA-256：`155cc18fb3fc0fb562c7145225a3ab602dabeadbacb7291d2dfecca8ae212d9c`。
- daemon SHA-256：`5f07824e32dc60cf76198dccffc00feaa9b9938f9a4a0163fe5e5eff0c4f9955`。
- 主程序、CLI、daemon、托盘的已安装文件与构建产物哈希一致。
- PID `68108`，可执行文件位于当前 bundle 的 `RemoventHost.app` 内。
- `running=true`、`host_ready=true`、`tray_connected=true`、屏幕录制和辅助功能权限为 true，`sessions=[]`。
- 登录启动保留为开启，服务 `managed=true`、`reachable=true`。
- 启动约 49 秒时，footprint 为 4,513 KiB，存活堆为 542 KiB。
- 随后 1,000 次独立连接的状态 IPC 请求全部返回共享就绪且无会话，用时 0.069 秒。请求前后 footprint 从 4,529 KiB 变为 4,577 KiB；再观察 120 秒，30、60、90、120 秒采样均为 4,593 KiB（约 4.49 MiB），没有继续增长。同一进程 RSS 为 16,544 KiB，两种指标未混用。

这是本机回归、隔离压力和短时运行验证；没有完成与原进程相同的约 46 小时观察、其他实体设备的远程连接或远端 CI。新旧进程运行时长不同，不能把重启后的内存下降比例当成长时间节省比例。未提交、推送或发布版本。
