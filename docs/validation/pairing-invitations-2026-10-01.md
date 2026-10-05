# 配对策略与主动连接码验证（2026-10-01）

后续整体安全审查与最新验证见 [认证与配对审查](authentication-review-2026-10-01.md)。

本次扩展被控端 desktop 的配对认证设置，并同步 desktop/iOS 控制端、daemon/CLI、
LAN Bonjour 与 QUIC/WebSocket relay。已有六位交互配对继续可用。

## 已实现的行为

- 默认记住已配对设备；可改为每次连接重新配对，快速恢复也不能绕过该策略。
- 被控端主动生成 12 位连接码，五分钟有效，成功验证一次后原子消费；替换/撤销立即失效。
- 控制端只输入连接码即可在 LAN 查找电脑；同一个 relay 内使用别名定位真实房间。
  relay 控制端凭据仍需通过，主机证书在 SPAKE2 配对成功后固定。
- 主动码生成即授权一次会话，无需第二次本地确认；拒绝所有连接、LoginWindow 预授权仍有效。
- CLI `pairing generate/show/watch/revoke` 经当前用户私有 daemon socket 操作；能显示六位待配对 PIN。
- 配对成功后保存及重连使用真实目的地；临时码不写入书签或同步记录。
- Bonjour/relay 仅发布六位公共 locator 与过期时间；六位秘密仅参与端到端 SPAKE2。
- iOS 使用系统 Bonjour；修复 Rust DNS-SD 实例名中的句点分隔、63 字节上限及本机回环，
  同一设备的多个接口/冲突后缀不会误判为 locator 冲突。

## 自动及实际验证

- Rust 相关 lib/bin 回归：307 passed，4 ignored；跳过已有
  `compatibility_protocols_resolve_and_remove_over_multicast`，其断言要求 native discovery 为空，
  与本机正在运行的 Removent 广播不兼容。其余 net 测试包含实际 multicast 广播、更新及销毁。
  日志：`target/pairing-unit-tests.log`。
- 完整 client session loopback：16 passed，覆盖四种认证、配对策略、过期/错误/复用码、
  真实协商、视频/输入、断线和恢复。额外三项 invitation 测试将再次询问准入设为拒绝，
  确认主动邀请无需第二次审批。日志：`target/pairing-session-tests.log`、
  `target/pairing-invitation-tests.log`。
- relay transport：14 passed，2 ignored（性能 benchmark），覆盖 QUIC/WSS 查码双向转发、
  真实房间返回、过期码拒绝、角色凭据、设备证明、证书绑定与 WSS 容器冷启动等待主机。
- Cloudflare Worker：TypeScript check 通过，14 tests passed；查码仍绑定原角色凭据及签名房间。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 真实隔离 daemon/CLI：generate、show、watch、revoke 全部通过；生成 12 位数字，私有邀请文件
  权限为 0600，撤销后 show 返回无有效码。证据：`target/removent-pairing-cli-5uhdo2bp/`。
- iOS 27 arm64 Simulator：10 unit tests 和一次完整邀请 UI 流程通过。
  UI 仅输入 12 位连接码，经真实系统 Bonjour 找到 synthetic Rust host，收到超过三帧解码视频、
  touch/Esc 四条输入到达 host；连接成功消费码，后台后重连无需输入 PIN；退出后验证保存了实际地址，并从书签再次连接成功，无需 PIN。
  结果：`target/mobile-pairing-review/Logs/Test/Test-RemoventMobile-2026.10.01_03-03-53-+0800.xcresult`；
  日志：`target/mobile-invitation-xcode-final.log`。

首次并行回归中的既有视频适配测试在解码阶段失败；隔离重跑及完整回归重跑均通过。
最终上述成功结果来自修复后的源码。

## 尚未验证的环境

未做物理 iPhone/WAN 验证，未向线上 Cloudflare 部署，未发布或安装这些改动。
iOS fixture 是真实 RVP 与视频/输入链路，画面来自 synthetic capture；不代表系统桌面捕获性能。
此次验证沿用 macOS 本地工具链，未执行 Windows/Linux 交叉构建。
