# Removent

[English](README.md)

Removent 是一个用 Rust 编写的 macOS 局域网远程桌面工具：在同一局域网内查看并控制另一台
Mac —— 无中转服务器、无账号，流量不出局域网。

- **为速度而生**：QUIC 传输、VideoToolbox 硬件 HEVC/H.264 编码、ScreenCaptureKit 采集、
  Opus 音频。
- **默认安全**：双向 TLS + 设备证书固定、SPAKE2 PIN 配对、逐连接的准入确认。
- **原生体验**：GPUI 桌面应用 + 菜单栏托盘托管常驻被控服务。

**当前状态**：早期开发阶段（已完成 M1 里程碑 —— 设备发现、配对、ping 与媒体管线可用；
仍有不少粗糙之处）。

## 系统要求

- macOS 13.0 或更高版本，Apple Silicon（arm64）
- 两台 Mac 处于同一局域网（通过 mDNS 自动发现）

## 快速开始

### 方式一：下载 DMG

从 [最新 release](https://github.com/removent/removent/releases/latest) 下载
`Removent-<版本>-macos-arm64.dmg`，打开后把 **Removent.app** 拖进「应用程序」。安装包
已经过 Developer ID 签名和 Apple 公证，打开时不会有 Gatekeeper 警告。

### 方式二：一行命令安装

```bash
curl -fsSL https://raw.githubusercontent.com/removent/removent/main/scripts/install.sh | bash
```

### 首次启动

macOS 会请求两项权限 —— 共享本机时都必需：

- **屏幕录制** —— 用于共享本机屏幕。
- **辅助功能** —— 用于本机被控制时注入键鼠输入。

在「系统设置 → 隐私与安全性」中授予后，从菜单栏托盘开启被控服务。在另一台 Mac 上从设备
列表找到本机（或手动输入 IP），输入被控端显示的配对 PIN，即可连接。

### Apple Remote Desktop / VNC 兼容

除自有的 RVP/QUIC 协议外，被控端可选开启标准 RFB/VNC 服务，供 VNC 客户端连接。该兼容
服务默认关闭；在设置页打开「Apple Remote Desktop / VNC」并设置密码
后，重启被控服务即可在 TCP `5900` 端口连接。密码为空时使用 RFB 的无认证模式，仅适合隔离
且可信的局域网。VNC 兼容层提供 Raw framebuffer 和键鼠输入，不提供 Removent 的配对、剪贴板、
音频、自适应编码等 RVP 功能。

在 Removent 主控端手动输入 `IP:5900` 可连接外部 VNC/屏幕共享服务器。标准 VNC 服务使用 VNC
密码；宣告 RFB `003.889` 的 Apple Remote Desktop / macOS「屏幕共享」服务使用设置中的 macOS
用户名和密码，并执行 Apple type-30/type-35 Diffie-Hellman/AES 认证。

## 从源码构建

需要 stable Rust 工具链（1.85+）和带 Swift 的 Xcode 命令行工具。

```bash
git clone https://github.com/removent/removent.git
cd removent

# 调试模式运行
cargo run -p removent-app

# 或打包 .app + zip 到 dist/（本地 ad-hoc 签名）
scripts/package.sh
```

测试：`cargo test --workspace`；托盘集成测试：`bash tray/Tests/run_integration_test.sh`。

### Benchmark

可重复运行 release 模式的编解码和控制链路测试：

```bash
cargo run --release -p removent-media-codec --example benchmark
cargo run --release -p removent-net --example rtt_benchmark
```

编解码测试输出 H.264/HEVC 编码及编码到解码回调的延迟、吞吐、压缩比和 Opus
编解码耗时；RVP 测试输出基于 QUIC `Ping/Pong` 的控制 RTT 百分位数。两条命令都会打印
构建模式和运行环境。

## 工作原理

三个进程协同工作：

- **Removent.app**（`crates/app`）—— GPUI 界面与主控端引擎（解码、渲染、输入转发）。
- **removentd**（`crates/daemon`）—— 常驻无头守护进程，承载被控端管线（采集、编码、
  输入注入），托盘通过 Unix socket IPC 管理它。
- **RemoventTray**（`tray/`，Swift）—— 菜单栏应用：服务开关、配对 PIN 展示、准入确认。

线上协议（"RVP"）基于 QUIC，使用双向 TLS 与 Ed25519 设备身份。完整设计文档见
[`.agents/`](.agents/README.md)（需求、架构、协议）。

## 自动更新

Removent 在启动 30 秒后、之后每 24 小时从 GitHub Releases 拉取一次
[`latest.json`](https://github.com/removent/removent/releases/latest/download/latest.json)
检查新版本。可在应用设置中关闭；该检查不影响纯局域网使用。

更新**仅提示、不强制**——有新版时设置页与菜单栏出现角标，不确认不会安装任何内容。
确认更新后，下载的包会先对照清单中的 SHA-256 与 Ed25519 签名（公钥已编译进应用）
校验通过才替换旧版本，旧版本会保留用于回滚。

## 国际化

界面支持英文和简体中文，默认跟随系统语言，可在应用设置中手动切换。面向贡献者的代码
注释与文档使用英文。

## 发布（维护者）

推送与 `Cargo.toml` 版本一致的 tag（如 `v0.1.0`）即可触发
`.github/workflows/release.yml`：构建、Developer ID 签名、公证，并把 DMG/zip/
`latest.json` 附加到 GitHub release。所需 secrets 见该 workflow 文件顶部注释。本地发布：
设置 `APPLE_SIGNING_IDENTITY` 和公证凭证（见 `scripts/notarize.sh`）后运行
`scripts/release.sh`。

## 开源协议

[Apache-2.0](LICENSE)
