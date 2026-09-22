---
title: "安装"
description: "在 Mac 上安装 Removent，为首次会话做好准备。"
order: 1
---

## 系统要求

- macOS 13 Ventura 或更新版本。
- Apple 芯片 Mac（arm64）。
- 原生连接需要另一台运行 Removent 的 Mac，两端在可达的局域网中，或已配置私有中继。
- VNC 或 RDP 连接需要已有的可达服务端，并使用受支持的身份验证方式。

## 从发布版本安装

打开 [GitHub Releases](https://github.com/backrunner/removent/releases)，选择最新已发布的 macOS arm64 DMG。可用版本与准确文件名以发布页为准。

打开 DMG，双击 **Removent.app → 安装并打开**，或将应用拖到 **Applications（应用程序）**。正式分发构建使用 Developer ID 签名与公证，本地临时签名包用于开发。

手动替换应用前，请退出主应用和菜单栏应用，并停止后台服务。安装器会拒绝替换正在运行的代码。已有安装建议使用应用内更新。

## 首次启动

从应用程序目录打开 Removent。如需共享这台 Mac，在菜单栏选择**设置屏幕共享权限…**。授予屏幕录制和辅助功能权限，然后重启后台服务。

接下来[配置权限](/docs/permissions)，然后[连接你的 Mac](/docs/connecting)。

## 更新

Removent 在启动后及之后定期检查更新，可在设置中关闭自动检查。检测到新版本时只会提示，安装需要由你主动发起。

更新器会验证清单中的 SHA-256 和 Ed25519 签名，再替换应用，并保留旧版本以便回滚。更新源使用 GitHub 最新稳定版，不提供预发布版本。

## 自行构建

如果发布页没有满足需求的版本，请参阅[从源码构建](/docs/building)。源码构建需要 Xcode 和 Rust 工具链。
