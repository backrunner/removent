# 认证与主动配对整体审查（2026-10-01）

已审查 desktop/iOS 控制端、被控端、daemon/CLI、LAN Bonjour、QUIC/WSS relay、
Cloudflare Worker 及相关配置与协议。发现的问题已修复，并为关键安全路径补充回归。
在以下验证范围内，没有剩余的已知阻塞问题；这不是对所有运行环境零缺陷的保证。

## 修复的问题

| 级别 | 问题与影响 | 修复 |
| --- | --- | --- |
| P1 | 控制端收到提前发送的 SessionAccept 后，可能跳过缺失或未通过的 PAKE Confirm；主动邀请码还可能被 peer_known/resume_accepted 声明跳过 | 主动码始终执行配对，客户端必须取得完整认证成功结果后才协商会话；恶意主机回归覆盖两个分支 |
| P1 | 已有限制权限的可信设备重新配对会被赋予全部权限，可能扩大 LoginWindow 访问 | 保留已有 granted_caps 和首次配对时间；新增测试覆盖每次配对与 LoginWindow 禁止扩权 |
| P1 | PairingCode 恢复令牌未普遍重查当前信任及权限，旧令牌可能绕过撤销 | 恢复握手按当前信任与能力范围重查准入；真实 QUIC 握手测试覆盖取消信任与收回输入权限 |
| P1 | 新认证消息以 PairingMsg 递归嵌套，可由恶意帧制造深递归解码 | 扁平 challenge/proof 结构，收发均限制认证帧至 4096 字节；超限测试确认在等待正文之前拒绝 |
| P1 | desktop 首次配对后缺少目的地址证书固定；移动端证书记录可能被覆盖或并发写入丢失 | desktop/mobile 共用原子、加锁的证书记录；迁移旧 mobile 文件；普通连接禁止覆盖，只有验证成功的显式主动码允许重新绑定 |
| P2 | DenyAll 仍会弹出认证输入，甚至消费 OTP 或更新配对状态后才拒绝 | 握手后立即拒绝；四种模式回归确保没有输入回调或 OTP 消费 |
| P2 | 主动码与当前六位 PIN 同时存在时 CLI 显示错码；失败/取消后旧 PIN 仍显示 | 优先显示当前六位 PIN，随后回退到仍有效的主动码；RAII 清理未完成提示，新增 pairing_cleared IPC 贯穿 daemon/desktop/CLI/tray |
| P2 | CLI 撤销或停用服务后其他管理界面保留配对码；生成与停用/修改认证之间存在竞态 | 清理广播同步管理界面；停用撤销邀请及恢复令牌；生成和配置变更使用同一设置锁串行化 |
| P2 | 托盘日志输出完整配对码，完成或取消后 modal 仍展示旧码 | 移除日志中的码；完成、撤销、断开或超时关闭对应配对提示 |
| P2 | WSS 冷启动允许格式错误的 pair- 别名进入等待 | 严格校验六位 ASCII 数字；异常别名回归确认立即拒绝 |

另修复 multicast 测试对“整个 native discovery 表必须为空”的错误假设。
现在检查唯一的测试服务不会进入 native 表，可与同机其他广播及并行测试共存，
本轮完整测试无需跳过该测试。

## 验证结果

| 验证 | 结果 | 证据 |
| --- | --- | --- |
| Rust 全工作区 lib/bin | 349 passed、0 failed、4 原有 ignored | `target/auth-review-unit-final.log` |
| 补充的真实恢复握手权限撤销回归 | 1 passed | `target/auth-review-resume-grants.log` |
| 完整 client session loopback | 19 passed | `target/auth-review-session-final.log` |
| QUIC/WSS relay tunnel | 15 passed、2 性能 benchmark ignored | `target/auth-review-relay-final.log` |
| Worker 测试 / TypeScript | 14 passed；check 通过 | `target/auth-review-worker.log`、`target/auth-review-worker-check.log` |
| 全工作区 all-targets clippy | `-D warnings` 通过 | `target/auth-review-clippy.log` |
| 后续 daemon / host / relay 改动 | 对应 all-targets clippy 通过；daemon 5 tests passed | `target/auth-review-daemon-clippy.log`、`target/auth-review-host-clippy.log`、`target/auth-review-relay-clippy.log`、`target/auth-review-daemon-final.log` |
| Swift tray | build 通过 | `target/auth-review-tray.log` |
| 实际隔离 daemon 与 CLI | generate/show/watch/revoke、停用失效、再次启用不恢复旧码通过 | `target/auth-review-cli.log`、`target/auth-review-cli-duekujqi/result.json`；复现脚本 `target/auth-review-cli.py` |
| iOS 27 Simulator + 实际 Rust host | 10 unit tests + 4 UI 场景通过 | 以下 xcresult 和日志 |
| Git diff | whitespace 检查通过 | `git diff --check` |

四个 iOS UI 场景使用实际 RVP/TLS/SPAKE2、系统 Bonjour、解码视频和主机输入记录：

- 主动码：只填连接码，通过 Bonjour 查找；消费一次；验证视频与输入；后台重连，
  保存实际地址后从书签再连接无需 PIN。
  `target/mobile-pairing-review/Logs/Test/Test-RemoventMobile-2026.10.01_04-28-32-+0800.xcresult`；
  `target/auth-review-ios-invitation.log`。
- 口令：安全输入框认证成功后验证视频与输入。
  `target/mobile-pairing-review/Logs/Test/Test-RemoventMobile-2026.10.01_04-30-04-+0800.xcresult`；
  `target/auth-review-ios-password.log`。
- OTP：读取实际当前 TOTP，输入后验证视频与输入。
  `target/mobile-pairing-review/Logs/Test/Test-RemoventMobile-2026.10.01_04-31-27-+0800.xcresult`；
  `target/auth-review-ios-otp.log`。
- 无认证：无需凭据提示，验证视频与输入。
  `target/mobile-pairing-review/Logs/Test/Test-RemoventMobile-2026.10.01_04-33-12-+0800.xcresult`；
  `target/auth-review-ios-none.log`。

四项原有 ignored 为线上 beta feed、实际 Keychain、libvncserver 外部 fixture 和 launchd 生命周期。
Rust 依赖 `block`/`proc-macro-error2` 仍有工具链 future-incompat 提示，当前 clippy 检查通过。
测试 fixture 与隔离 daemon 已停止。

## 验证边界与兼容性

- 六位传统配对消息的 postcard 序号与结构保留；新认证变体采用扁平结构，
  本次尚未发布的 host/controller 应一起更新；口令和 OTP 不支持旧控制端。
- 验证使用 macOS 本地工具链及 iOS Simulator；未做物理 iPhone、Windows/Linux 交叉构建、
  WAN 或线上 Cloudflare 部署。本轮未安装发布包或提交推送。
- iOS 验证画面为 synthetic capture，证明真实网络、认证、解码与输入通路，不证明系统屏幕捕获性能。
- 实际 tray 编译与状态事件处理已检查；未进行原生托盘点击和 modal 视觉验收。
