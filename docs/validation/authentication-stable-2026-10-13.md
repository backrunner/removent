# 0.1.4 认证与连接协商验收

日期：2026-10-13，macOS 本地工具链，iOS 27 Simulator。正式发布版本为 `0.1.4`，移动端源码版本为 `0.1.4`（5），RVP 仍为协议 v1。

## 实现核对

- Desktop 与移动端均调用 `removent-client` 的生产 RVP 会话引擎，声明 `AUTH_METHODS`，按主机 challenge 中的模式向各自 UI 请求凭据。移动端 JSON/Swift 桥保留模式和会话 generation；旧事件不能消耗新会话的凭据。
- 配对码、访问口令、OTP 使用 SPAKE2、绑定设备身份的 transcript 与双方签名确认。客户端收到 `SessionAccept` 后仍需等待密码学确认，才能进入媒体与输入协商。OTP 有时间窗口及持久化防重放状态。
- 主机分别处理配对码、口令、OTP、无认证；口令与 OTP 对已知设备也重新认证，无认证不弹出凭据输入，但保留 RVP 加密与主机证书验证。拒绝传入连接先于凭据提示和 OTP 消耗执行。
- 主机要求口令或 OTP 时，缺少 `AUTH_METHODS` 的旧控制端被拒绝。认证策略或凭据改变、撤销信任、缩减权限以及每次配对策略均不能通过旧恢复 token 绕过。
- QUIC relay 证书确认、WebSocket TLS 校验、relay 访问凭据和注册设备签名独立于端到端主机认证。首次 QUIC relay 信任确认先于凭据发送。relay 不能把已固定证书的控制端重定向至其他主机。
- VNC/ARD 与 RDP 两端共用生产客户端：VNC 支持 None、VNC DES 口令、Apple ARD DH/AES 账户认证的安全类型选择；RDP 支持 TLS 与 CredSSP/NLA。未扩展未实现的第三方认证类型。

## 新增覆盖

`packages/client/tests/session_loopback/authentication.rs` 使用真实 QUIC endpoint、真实 relay、生产主机与客户端握手，遍历四种主机模式及 QUIC/WebSocket 两个载体。relay 同时启用访问凭据和设备公钥限制；主机口令与 relay 凭据不同，relay 凭据不能代替主机口令，错误的六位 OTP 被拒绝。配对必须创建配对授权，无人值守模式不得生成配对授权或触发主机配对/批准提示。

移动端增加 relay 认证 UI 场景。测试在 relay 凭据之后依次确认 relay 和主机证书，再输入主机所需的配对码、口令或 OTP。成功后确认实际解码帧数，并检查输入事件到达 Rust 主机。

## 本地结果

| 检查 | 结果 |
| --- | --- |
| `cargo test --locked --workspace -- --test-threads=1` | 467 passed，0 failed，7 ignored |
| 最终认证回归（包括 relay 矩阵） | 6 passed，0 failed |
| workspace strict Clippy，格式，布局，diff | 通过 |
| 发布/构建/relay/macOS 脚本测试 | 58 项，0 failed，1 skipped |
| iOS 单元测试 | 20 passed，0 failed |
| iOS 直连四种认证模式 | 4 个 UI 场景通过，实际解码画面与输入 |
| iOS QUIC relay 四种认证模式 | 4 个 UI 场景通过，连续认证、画面与输入 |
| iOS 一次性邀请 | 1 个 UI 场景通过，视频、输入、邀请消耗及记住主机后重连 |
| iOS Simulator `build-for-testing` | 通过 |
| iOS arm64 Release Rust 库与完整 App | 通过，App 标识及版本核对为 `com.alkinum.removent.mobile`，`0.1.4`（5） |
| Swift tray / Apple Cloud Sync | 6 / 5 项通过 |
| 官网类型检查、文档、构建、浏览器 | 0 errors / warnings，32 个 Chromium/WebKit 场景通过 |
| 官网 support 测试与 Worker dry-run | 通过 |
| iOS 素材元数据检查（templates-only） | 通过；素材测试已与实际发布版本隔离 |

主要日志位于 `/tmp/removent-014-workspace.log`、`/tmp/removent-014-relay-auth-final.log`、`/tmp/removent-014-ios-unit.log`、`/tmp/removent-014-ios-matrix.json` 和 `/tmp/removent-014-ios-extra.json`。移动端原生截图及测试结果位于 `target/mobile-stable-014/Logs/Test/*.xcresult`。已检查 relay 口令提示的原生截图。

## 边界

测试视频为编码后通过真实 RVP 网络传输的 synthetic capture，输入记录器不控制本机。WebSocket 矩阵使用 loopback WS 载体，内部 RVP 仍加密；WSS 拒绝不可信证书由独立 relay 测试覆盖。本次没有物理 iPhone/iPad、WAN、线上 relay 或第三方 RDP/ARD 服务器验收，也没有安装正式包后的桌面 UI 点击验收。

七项 Rust ignored 保留为真实 Keychain、外部 libvncserver、GUI/launchd 生命周期、线上 beta feed 和两项性能基准。iOS 真机架构 App 使用未签名构建，仅证明编译与链接；本次 GitHub 发布不等同于 App Store 上传、提审或批准。CloudKit Production 与上游依赖审计边界沿用已有报告。

远端 CI、Relay、Release、签名、公证及公开产物验证须针对正式 tag 的同一提交完成；本地结果不替代这些发布检查。
