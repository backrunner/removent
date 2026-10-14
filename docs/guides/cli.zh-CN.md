# CLI 指南

[English](cli.md) | [返回 Removent](../../README.zh-CN.md)

随应用提供的 CLI 管理本机设置与后台服务，与桌面端和 daemon 使用同一份私有数据目录和设置锁。

```sh
REMOVENT_CLI="/Applications/Removent.app/Contents/MacOS/removent-cli"
"$REMOVENT_CLI" --version
"$REMOVENT_CLI" --help
"$REMOVENT_CLI" settings --help
```

用户目录安装时，将 `/Applications` 换为 `$HOME/Applications`。
源码构建使用 `target/debug/removent-cli`，默认数据目录为 `userdata`；`scripts/dev.sh` 则使用
`userdata/dev`。管理自定义或开发实例时，应明确设置 `REMOVENT_DATA_DIR`。

## 查询与修改

```sh
"$REMOVENT_CLI" settings list                    # 设置名、当前值、可用值和生效方式
"$REMOVENT_CLI" settings get                     # 全部当前值
"$REMOVENT_CLI" settings get host_port           # 一项当前值
"$REMOVENT_CLI" settings path                    # 当前 settings.toml 路径
"$REMOVENT_CLI" settings set device_name "Office Mac"
"$REMOVENT_CLI" settings set discovery.vnc true
"$REMOVENT_CLI" settings set theme dark
"$REMOVENT_CLI" settings set update_channel beta
"$REMOVENT_CLI" settings reset theme             # 只重置这一项
```

布尔值接受 `true`/`false` 或 `on`/`off`。枚举值忽略大小写、连字符和下划线，
例如 `high-quality` 对应 `HighQuality`，`zh-CN` 对应 `ZhCn`。空字符串使用 `""`。

每次修改都会在共享文件锁内重新读取最新设置，校验后再原子替换配置文件，文件权限为 `0600`。
无效值不会覆盖原配置。配置文件损坏时会报错，不会移动、重置文件，也不会回显其中的内容。

## 设置项

| 设置名 | 可用值 | 生效方式 |
| --- | --- | --- |
| `device_name` | 非空文本，最多 63 个 UTF-8 字节 | 后台重新加载；桌面端显示名称需重新打开后刷新 |
| `language` | `System`、`En`、`ZhCn` | 重新打开桌面端；下一条 CLI 命令使用新语言 |
| `theme` | `System`、`Light`、`Dark` | 重新打开桌面端 |
| `discovery.removent` | 布尔值 | 重新打开桌面端 |
| `discovery.vnc` | 布尔值 | 重新打开桌面端 |
| `discovery.rdp` | 布尔值 | 重新打开桌面端 |
| `host_enabled` | 布尔值 | 后台重新加载 |
| `host_port` | `1`–`65535`，UDP | 后台重新加载 |
| `admission` | `AlwaysAsk`、`TrustedAuto`、`DenyAll` | 后台重新加载 |
| `paired_only` | 布尔值 | 后台重新加载 |
| `authentication.mode` | `system_account`、`pairing_code`、`password`、`otp`、`none` | 后台重新加载 |
| `authentication.pairing_policy` | `remember_device`、`every_connection` | 后台重新加载 |
| `authentication.password` | 最多 1024 字节的文本，通过 `--stdin` 写入 | 后台重新加载 |
| `authentication.otp_secret` | 有效 Base32 密钥，通过 `--stdin` 写入 | 后台重新加载 |
| `window_server_capture` | 布尔值，按需启用旧版采集路径 | 后台重新加载 |
| `vnc_enabled` | 布尔值 | 后台重新加载 |
| `vnc_port` | `1`–`65535`，TCP | 后台重新加载 |
| `vnc_password` | 最多 1024 字节的文本，通过 `--stdin` 写入 | 后台重新加载 |
| `audio_enabled_default` | 布尔值 | 重新打开桌面端 |
| `clipboard_clear_after_secs` | `0`–`4294967295`，`0` 表示不自动清空 | 重新打开桌面端 |
| `video_quality` | `Auto`、`Smooth`、`Balanced`、`HighQuality`、`Extreme` | 重新打开桌面端 |
| `update_check_enabled` | 布尔值 | 重新打开桌面端 |
| `update_channel` | `stable`、`beta` | 重新打开桌面端 |
| `update_endpoint` | 不含凭据和片段的 HTTPS 地址，留空使用默认地址 | 重新打开桌面端 |

`settings` 管理本地偏好；登录自启和系统权限使用下方的 `daemon` 命令。
中继路由与 iCloud 签名部署使用独立配置，见[中继指南](private-relay.md)和
[iCloud 设计](../architecture/cloudkit-sync-design.md)。

## 密码与 OTP

`list`、`get`、`set` 和 JSON 输出始终隐藏密码及 OTP 密钥，只显示是否已配置。
敏感值必须从标准输入传入，避免出现在进程参数中：

```sh
"$REMOVENT_CLI" settings set authentication.password --stdin < /private/path/password.txt
"$REMOVENT_CLI" settings set authentication.mode password
```

输入必须为 UTF-8 文本，最多 4096 字节；每个设置项仍有独立的长度限制。
输入末尾的一组换行会被移除，空格原样保留。`--stdin` 需要管道或文件，不从交互式终端读取。

OTP 模式需要先将同一个 Base32 密钥配置到验证器，再通过 `authentication.otp_secret --stdin`
写入，最后将 `authentication.mode` 切换为 `otp`。桌面端安全设置可以生成密钥和绑定二维码。

重置当前认证方式所需的密码或 OTP 密钥前，先切换到 `pairing_code` 或 `system_account`。
CLI 会拒绝不完整的认证配置。`system_account` 使用被控 Mac 的系统账号验证，不通过这些设置保存系统密码。
`none` 允许不带凭据连接，只有这正是你需要的访问策略时才使用。空 VNC 密码表示无认证 VNC。

## 何时生效

共享设置保存后，会向在线 daemon 发送 `ReloadSettings`。名称、端口、共享开关和认证策略等变化
会按需重启共享监听器，可能中断正在进行的远程会话；认证策略变化还会清除配对邀请和会话恢复状态。
只修改桌面偏好不会重启共享服务。

后台服务离线时仍可保存，设置在下次启动时生效。这不会启动服务，也不会清除用户手动停止服务的记录。
若 daemon 拒绝重新加载，或未及时确认，CLI 会明确提示**设置已保存**，返回退出码 `1`，
并给出 `daemon reload` 重试命令。重新加载确认不等于监听器已就绪，
可通过 `daemon status` 的 `host_ready` 和 `host_error` 检查实际状态。

## JSON 与退出码

所有设置命令均支持 `--json`。`list` 和不指定设置名的 `get` 返回设置记录数组；
`get <设置名>` 返回一条记录，包含 `key`、`value`、`type`、`accepted`、`secret` 和 `apply`。

敏感值返回 `{"redacted":true,"configured":true}`。保存结果包含 `saved`、`setting` 和 `application`；
`application` 可为 `desktop_restart`、`daemon_offline`、`reload_requested` 或 `reload_failed`。

```sh
"$REMOVENT_CLI" settings get host_port --json
"$REMOVENT_CLI" settings set host_port 48688 --json
```

| 退出码 | 含义 |
| --- | --- |
| `0` | 操作成功；离线保存设置也可成功 |
| `1` | 运行失败；设置可能已保存，但重新加载失败 |
| `2` | 命令、设置名、值无效，或认证配置不完整 |

服务、配对和身份命令保留现有 JSON 输出。诊断日志默认写入本地轮转文件，错误写到 stderr。
加上 `--verbose` 可在终端同时查看日志和底层错误原因，`RUST_LOG` 控制日志级别。

## 服务与配对

```sh
"$REMOVENT_CLI" daemon status          # 共享、监听器、会话与权限状态
"$REMOVENT_CLI" daemon service-status  # 进程可达性与登录自启状态
"$REMOVENT_CLI" daemon start
"$REMOVENT_CLI" daemon stop
"$REMOVENT_CLI" daemon restart
"$REMOVENT_CLI" daemon enable          # 保存共享开启状态
"$REMOVENT_CLI" daemon disable         # 关闭共享，保留 IPC
"$REMOVENT_CLI" daemon reload          # 重新加载已保存的设置
"$REMOVENT_CLI" daemon login-on
"$REMOVENT_CLI" daemon login-off
"$REMOVENT_CLI" daemon permissions     # 发起系统隐私权限请求
"$REMOVENT_CLI" pairing generate  # 需使用配对码认证，并已开启共享
"$REMOVENT_CLI" pairing show
"$REMOVENT_CLI" pairing watch          # 每行一个 JSON，Ctrl+C 结束
"$REMOVENT_CLI" pairing revoke
```

服务启停和登录自启使用 macOS launchd，生命周期与权限设置详见
[后台服务指南](macos-background-service.md)。直连和私有中继的连接测试见 `ping --help`；
本机公钥与证书指纹可通过 `identity` 查询。当前 `ping` 会话探测仅支持配对码或无认证的被控端；
默认系统账户、访问口令和 OTP 模式请使用桌面端或移动端验证连接，`ping` 不会绕过认证。
