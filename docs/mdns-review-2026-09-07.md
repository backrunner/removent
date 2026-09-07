# mDNS 自发现失败排查与修复

## 结论与证据

mDNS 用于局域网设备自动发现。发现失败会导致设备列表缺失，不能当作正常失败忽略；发现失败本身不等于 QUIC/VNC/RDP 连接失败。

原 `advertise_and_browse_loopback` 测试虽然名为 loopback，却直接使用默认网卡配置。mdns-sd 0.13.11 默认禁用回环接口，因此测试实际上依赖当前局域网组播环境。

本机诊断发现：

- 浏览器没有收到 ServiceFound/ServiceResolved，PTR/SRV/TXT/地址缓存计数均为零。
- 已启动的发现接口只有 IPv6，缺少两张有 IPv4 地址的活动局域网网卡。
- 独立 Python UDP 探针复现：绑定 5353、加入 224.0.0.251 组播和设置出口接口均成功，但两张局域网网卡的发送都返回 `No route to host`（macOS error 65）。相同探针在 127.0.0.1 上发送成功。
- mdns-sd 在初始化 IPv4 接口时试发组播；发送失败会跳过该接口。因此 daemon 创建成功不代表局域网发现一定可用。

这定位了当前测试失败的直接原因，但尚未区分导致系统拒绝发送的具体网络权限、VPN、过滤规则或路由问题。IPv6 无接收记录的原因也未单独证实。没有修改系统权限、VPN、路由或防火墙。

## 修复

- 测试显式禁用其他接口并启用 IPv4 回环；广告端、浏览端仍使用两个独立 daemon，通过真实 UDP 组播通信。没有忽略测试或用内存缓存模拟网络。
- 测试使用随机实例标识，验证名称、地址、端口、能力、四次忙碌状态更新以及退出后的 goodbye 移除。
- 修复 `Advertiser::set_busy`：直接重新注册同名服务更新 TXT。原实现先 unregister，会发送 goodbye 和延迟重发，可能在更新期间或更新后让设备从对端缓存消失。回归测试同时检查状态更新期间可见性和 unregister 计数，避免 watch 合并事件掩盖问题。
- 浏览器记录实际启用接口的 debug 日志，并停止在查询启动等未改变设备表的事件上复制和发布整张表。
- 打包 Info.plist 补齐 `NSLocalNetworkUsageDescription`、`NSBonjourServices = ["_removent._udp"]`，并提供中文权限说明。这完善发布包声明，不会自动授予权限，也不改变从终端运行的测试进程权限。

生产环境仍使用默认局域网接口，回环限定只用于测试。mdns-sd 已每 5 秒重新检查接口，并重试未成功绑定的已选接口，无需再增加重复重试线程。

## 验证

- 专项回环测试通过；全量运行中全部 5 项 discovery 测试、全部 10 项 net 单元测试通过。
- 全量测试首次运行：228 passed、4 failed。四项失败均在 client VNC 测试的 TCP connect 阶段返回 `AddrNotAvailable`（macOS error 49），与本轮 mDNS 改动的 UDP 发现路径不同；之前也曾出现这种间歇性失败。
- 对失败路径执行 `cargo test --workspace --no-fail-fast -- vnc::tests:: --test-threads=1` 定向复跑，全部 17 项通过（包含首次失败的四项）。本轮没有修改 VNC，也没有通过自动重试掩盖其失败；完整记录保留在 `/tmp/removent-mdns-vnc-recheck.log`。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- `cargo fmt --all --check`、`git diff --check`、`bash -n scripts/package.sh` 通过。
- 提取打包脚本生成的 Info.plist，通过 plistlib 解析并验证服务类型和权限文案；未执行完整 release 打包。

日志：`/tmp/removent-mdns-diagnostic.log`、`/tmp/removent-mdns-loopback.log`、`/tmp/removent-mdns-full.log`、`/tmp/removent-mdns-clippy.log`。

## 真实局域网验收

回环通过不能替代两台设备的验收。仍需在允许本地网络访问、支持组播的同一局域网验证：互相出现、忙碌状态更新不消失、退出移除、网络断开后恢复发现。

排查实际机器时，检查 macOS「隐私与安全性 → 本地网络」中实际运行的应用/终端的权限，以及 VPN 或网络设备是否限制 UDP 5353 / 224.0.0.251 / ff02::fb。可用 `RUST_LOG=info,mdns_sd=trace,removent_net::discovery=debug` 获取接口绑定和查询日志。日志中没有 IPv4 接口时，应先排查底层组播发送失败，而不是继续增加发现超时。
