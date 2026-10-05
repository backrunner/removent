# 原生远程连接认证方式验证（2026-10-01）

后续整体安全审查与最新验证见 [认证与配对审查](authentication-review-2026-10-01.md)。

Desktop 被控端新增「设置 → 安全 → 连接认证方式」，支持配对码（默认）、访问口令、
验证器 OTP 和无认证。iPhone/iPad app 与 desktop 主控端由远端 Challenge 决定输入方式。
口令、OTP、无认证无需现场准入确认；「允许连接」关闭时仍拒绝所有连接。

## 实现边界

- 配对码保持旧协议消息编号和持久授权行为；新消息追加在原有 enum 之后。
- 口令与 OTP 使用 SPAKE2，确认绑定两端 TLS 证书指纹及设备签名；不发送明文凭据。
- OTP 使用 RFC 6238 的六位 / SHA-1 / 30 秒规则，允许前后一个周期的时钟偏差；
  验证时再检查有效期。成功使用的周期写入 `otp-used.json`，重启、重连或更换设备不能重用。
- 口令/OTP 认证开始受被控端全局五秒限流，设备身份轮换不能绕过。
- 已配对设备在口令/OTP 模式下仍必须验证。三种无人值守模式不会新增配对信任或长期能力授权。
- 无认证允许所有可到达的 Removent 主控端连接与控制；TLS 加密仍保留。
- Desktop 可复制 OTP 密钥，手动绑定验证器。设置和凭据只存于权限为 0600 的本地配置，不参与 iCloud 同步。
- 修改认证方式、口令或 OTP 密钥会通过 daemon ReloadSettings 通知监听器重启，
  结束现有连接并清除旧恢复记录；恢复令牌另绑定认证配置指纹。
- relay 的路由凭据独立；系统级 LoginWindow 服务的预先授权限制保持有效。
- 密码和 OTP 要求主控端声明 `AUTH_METHODS` 能力；旧主控端仍可用默认配对码。

## Rust 验证

`cargo check --workspace --all-targets` 通过。
相关包的 `cargo clippy --all-targets -- -D warnings` 通过；Cargo 仍报告已有依赖的 future-incompatibility 提示。

通过的专项测试包括：

- Desktop 设置切换四种方式、持久化、拒绝空口令及小窗口下的选择控件布局。
- Core 旧配置默认配对、口令持久化、凭据 Debug 隐藏、RFC 6238 标准向量与 OTP 重放记录持久化。
- Mobile Rust 输入按认证方式验证，旧连接 generation 不能消费当前输入通道。
- Host 修改认证配置/凭据后旧恢复策略不再匹配。
- Daemon 重新加载认证设置会通知 runner，保持 host_enabled 原值；普通名称编辑不触发重启。
- Client 真实 QUIC 回环：正确口令/OTP成功，已知设备仍需口令，错误口令/OTP及过期/重复 OTP失败；
  无认证无需输入且允许未知设备，DenyAll拒绝，取消输入及时结束。无人值守模式不创建配对授权。
- 原有 session_loopback 的全部 13 项通过，包括配对、准入、媒体、忙碌拒绝、取消、快速恢复和异常重连。
- Net 配对原语与旧 Confirm 编码编号测试通过。

较大范围测试中，一个已有发现测试
`discovery::tests::compatibility_protocols_resolve_and_remove_over_multicast` 失败：
其断言 Removent 发现表完全为空，当前测试环境中存在 Removent 广播设备。
排除该项后 Net 的其余 25 项通过。该测试未因本次认证需求而修改。

## iOS 模拟器验证

环境：Xcode，iOS 27.0 模拟器 `Removent-Mobile-Review`，synthetic loopback host。
手机接收真实 Rust 解码画面，键鼠输入写入 fixture 的 recorder，不控制实际电脑。

9 项 Swift 单元测试通过，包括远端认证类型和过期事件检查。

| 模式 | 界面及连接结果 | 结果 bundle |
| --- | --- | --- |
| 口令 | SecureField 输入，连接后收到多帧，点击和 Esc 到达主机 | `target/mobile-auth-review/Logs/Test/Test-RemoventMobile-2026.10.01_01-29-06-+0800.xcresult` |
| OTP | 六位动态验证码输入，连接后收到多帧，点击和 Esc 到达主机 | `target/mobile-auth-review/Logs/Test/Test-RemoventMobile-2026.10.01_01-30-29-+0800.xcresult` |
| 无认证 | 无凭据输入界面，直接收到多帧，点击和 Esc 到达主机 | `target/mobile-auth-review/Logs/Test/Test-RemoventMobile-2026.10.01_01-25-32-+0800.xcresult` |
| 配对码 | 配对、画面、鼠标及软件键盘输入、后台返回重连、横屏布局 | `target/mobile-auth-review/Logs/Test/Test-RemoventMobile-2026.10.01_01-31-58-+0800.xcresult` |

上述结果均有 `TEST SUCCEEDED`。口令和 OTP 输入界面截图已导出并人工查看：
`target/mobile-auth-review/attachments/password-final/` 与 `otp-final/`。
早期两次测试虽完成断言，但 Xcode 卡在自动诊断收集；停止后用
`-collect-test-diagnostics never` 重跑，以上口令和 OTP bundle 为完成且可读取的结果。

复现：

```sh
REMOVENT_FIXTURE_AUTH=password cargo run -p removent-host --example mobile_fixture
# 可改为 otp 或 none；不设环境变量则为默认配对码。
xcodebuild -project apps/mobile/RemoventMobile.xcodeproj -scheme RemoventMobile \
  -destination 'platform=iOS Simulator,name=Removent-Mobile-Review' \
  -only-testing:RemoventMobileUITests/ConnectionTests/testUnattendedAuthenticationVideoAndInput \
  -parallel-testing-enabled NO -collect-test-diagnostics never test
```

本轮未验证物理设备、真实 WAN、系统 LoginWindow 交互或打包后的安装流程。
