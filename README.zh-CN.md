<p align="center">
  <img src="apps/website/static/app-icon.png" width="104" alt="Removent 应用图标">
</p>

<h1 align="center">Removent</h1>

<p align="center">
  连接你的 Mac，也能从 iPhone 和 iPad 远程操作。<br>
  局域网直连，或通过自己部署的中继跨网络连接，无需托管账号。
</p>

<p align="center">
  <a href="https://github.com/backrunner/removent/releases"><img src="https://img.shields.io/github/v/release/backrunner/removent?style=flat-square&color=4466dd" alt="最新版本"></a>
  <img src="https://img.shields.io/badge/macOS-26%2B-333333?style=flat-square" alt="macOS 26 或更高版本">
  <img src="https://img.shields.io/badge/Apple_Silicon-arm64-333333?style=flat-square" alt="Apple Silicon">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-333333?style=flat-square" alt="Apache 2.0 许可证"></a>
</p>

<p align="center">
  <a href="https://removent.pwp.sh/zh">官网</a> |
  <a href="https://github.com/backrunner/removent/releases">下载</a> |
  <a href="docs/guides/cli.zh-CN.md">CLI 指南</a> |
  <a href="README.md">English</a>
</p>

<p align="center">
  <img src="docs/images/desktop-settings.png" width="960" alt="Removent macOS 实际界面，展示设置侧栏和深色外观预览">
  <br><sub>macOS 应用实际截图，界面支持简体中文与英文。</sub>
</p>

## 可以做什么

| | |
| --- | --- |
| **远程操作 Mac** | RVP 原生会话支持键鼠、剪贴板和音频，使用 QUIC 传输与 HEVC/H.264 硬件视频编码。 |
| **从手机和平板连接** | [SwiftUI 控制端](apps/mobile/README.md)与桌面端共用 Rust 连接引擎，需要 iOS/iPadOS 26 或更高版本。 |
| **连接已有服务器** | 在同一应用中连接 VNC、Apple Remote Desktop 和 RDP 服务。 |
| **保持后台共享** | 后台服务与菜单栏应用独立运行，关闭主窗口后仍可提供共享。 |
| **跨网络使用** | 在自己的 VPS 上运行 Rust 中继，或部署到 Cloudflare Containers。 |
| **通过命令行管理** | 使用随应用提供的 CLI 查询状态、修改设置、管理配对和登录自启。 |

Removent、VNC 和 RDP 的 Bonjour 发现可分别开启。发现仅使用服务广播，不扫描子网或端口；
没有广播的电脑可以手动输入地址添加。

## 安装

**Mac 应用要求：** macOS 26 或更高版本，Apple Silicon。

从 [GitHub Releases](https://github.com/backrunner/removent/releases) 下载 DMG，打开后选择
**Removent → 安装并打开**，也可以拖入「应用程序」。

也可在终端中安装：

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash
```

安装脚本在替换前检查发布包校验和、各组件签名和 Gatekeeper，并保留旧应用以便失败时回滚。
升级前请结束远程会话，并退出主窗口和菜单栏应用。

<details>
<summary>通过 SSH 安装</summary>

将 `user@mac` 替换成目标 Mac 的 SSH 地址：

```sh
ssh user@mac 'curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash -s -- --app-dir "$HOME/Applications" --login'
```

`--login` 会启动服务并启用用户登录自启；`--start` 只启动服务；省略两者则仅安装。
共享需要该用户已有已登录的图形会话，首次隐私授权仍需在 Mac 的系统设置中完成。
详见 [SSH 安装指南](docs/guides/ssh-install.md)。

</details>

## 建立第一次连接

1. **准备被控 Mac。** 打开 Removent，授予屏幕录制和辅助功能权限，然后开启共享。
2. **在另一台设备选择它。** 从附近设备中选择，或点击「添加连接」手动输入地址。
3. **完成认证。** 按被控端的安全设置，输入系统账号、配对码、访问口令或验证器中的验证码。

被控端也可以生成一次性的 12 位连接码。在同一局域网的控制端地址栏输入即可连接，
也可通过两端共同使用的私有中继连接。连接码五分钟后过期。

需要关闭主窗口后继续提供共享，可查看[后台服务设置](docs/guides/macos-background-service.md)。

## 使用 CLI

CLI 位于 Mac 应用包内。先在当前终端设置路径：

```sh
REMOVENT_CLI="/Applications/Removent.app/Contents/MacOS/removent-cli"
```

```sh
# 非全屏键盘菜单
"$REMOVENT_CLI"

# 服务与配对
"$REMOVENT_CLI" daemon status
"$REMOVENT_CLI" daemon start
"$REMOVENT_CLI" pairing generate  # 需使用配对码认证，并已开启共享

# 查询、修改与重置设置
"$REMOVENT_CLI" settings list
"$REMOVENT_CLI" settings get discovery.vnc
"$REMOVENT_CLI" settings set device_name "Office Mac"
"$REMOVENT_CLI" settings set discovery.vnc true
"$REMOVENT_CLI" settings set video_quality high-quality
"$REMOVENT_CLI" settings reset video_quality

# 脚本输出与诊断
"$REMOVENT_CLI" settings get --json
"$REMOVENT_CLI" daemon status --verbose
```

终端默认显示易读状态，管道保留原有输出格式。使用 `--json` 获取机器可读结果，
或用 `--human` 在重定向时保留易读文本。无参数运行可通过键盘管理服务、配对和设置。

设置命令覆盖外观、设备发现、共享、认证、VNC、媒体偏好和更新，保存前会校验输入，
并说明生效方式。后台服务在线时会重新加载共享设置，可能中断正在进行的远程会话；
桌面偏好在重新打开桌面端后生效。

查询结果始终隐藏密码和 OTP 密钥。写入敏感值使用 `--stdin`，无需将它们放进命令参数。
全部设置项、认证配置、JSON 格式和退出码见 [CLI 完整指南](docs/guides/cli.zh-CN.md)。

## 选择连接协议

| 协议 | 用途 | 被控端要求 |
| --- | --- | --- |
| **Removent / RVP** | Mac 之间或移动端到 Mac 的连接，原生媒体与私有中继 | Mac 已开启 Removent 共享 |
| **VNC / Apple Remote Desktop** | 连接已有 VNC 服务或 macOS 屏幕共享 | 提供受支持的 RFB 认证方式 |
| **RDP** | 连接 Windows 等 RDP 桌面 | 已开启远程桌面，账号具有登录权限 |

Mac 可额外提供 VNC 共享，默认关闭；RDP 目前仅支持作为控制端连接。
不同协议的媒体与重定向能力有所不同，认证方式、编码选项和限制见
[连接参考](docs/guides/connection-reference.md)。

## 部署自己的中继

提供 Linux x86_64/ARM64，以及 macOS 13+（Apple Silicon 和 Intel）的预编译中继：

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

安装器会校验程序并打开配置向导。Linux 使用 systemd，macOS 使用用户 LaunchAgent。
可通过 `removent-relay start`、`stop`、`restart`、`status` 和 `logs` 管理服务。

[中继安装](docs/guides/relay-quick-deploy.md) |
[私有中继配置](docs/guides/private-relay.md) |
[Cloudflare 部署](docs/guides/cloudflare-relay.md)

## 认证与后台共享

原生连接使用双向 TLS、设备证书固定与 SPAKE2 配对。被控端可选择系统账号、配对码、访问口令、
验证器 OTP 或无认证。选择无认证时，任何能访问被控端的人都能连接。
凭据和配对权限保留在各自设备上。

常规共享运行在已登录的 macOS 图形会话中。FileVault 启动前环境与初始登录窗口另有要求，
详见 [macOS 解锁说明](docs/guides/macos-unlock.md)。

iCloud 已保存连接同步为可选功能，需要相应签名配置和 CloudKit 部署。
参见[同步架构](docs/architecture/cloudkit-sync-design.md)和[验证状态](docs/validation/cloudkit-sync-validation.md)。

## 开发与构建

需要当前稳定版 Rust，以及包含 Swift 和 Metal 工具的完整 Xcode。

```sh
git clone https://github.com/backrunner/removent.git
cd removent
scripts/dev.sh                  # 构建并运行桌面端、后台服务与托盘
scripts/dev.sh --no-build       # 使用已有调试程序
scripts/build/package.sh       # 在 dist/ 中生成本地 .app 与 zip
```

开发数据默认位于 `userdata/dev`，可通过 `REMOVENT_DATA_DIR` 指定其他目录。
按 Ctrl+C 会停止开发脚本启动的进程。未配置签名身份时，本地打包使用 ad-hoc 签名。

```sh
make check
make test
make website-check
```

| 目录 | 内容 |
| --- | --- |
| `apps/` | 桌面端、后台服务、CLI、托盘、移动控制端、中继和网站 |
| `packages/` | 共享设置、身份、传输、媒体、输入与连接引擎 |
| `infra/` | 部署和服务模板 |
| `docs/` | 使用指南、架构和验证记录 |

[仓库结构](docs/architecture/monorepo.md) |
[设计文档](docs/design/README.md) |
[发布指南](docs/guides/release-pipeline.md)

## 许可证

[Apache-2.0](LICENSE)。
