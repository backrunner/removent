---
title: "快速开始"
description: "在局域网直接连接你的 Mac，或通过自己的中继跨网络访问。"
order: 0
---

Removent 是面向 Apple 芯片 Mac 的远程桌面，要求 macOS 13 或更新版本。一个应用即可使用原生 Removent 连接、VNC 查看器和 RDP 客户端。项目目前处于早期开发阶段。

## 第一次连接

1. 在两台 Mac 上[安装 Removent](/docs/installation)。
2. 在被控 Mac 上[授予托管权限](/docs/permissions)，并从菜单栏启用主机服务。
3. 在控制端打开 Removent，从设备列表选择被控 Mac，或手动添加地址。
4. 输入被控端显示的 PIN，并批准请求的访问权限。

配对后，受信任设备可以在已批准的能力范围内重新连接。[查看完整连接流程](/docs/connecting)。

## 选择协议

| 连接方式 | 使用场景 | 准备条件 |
| --- | --- | --- |
| Removent（RVP） | 原生 Mac 之间的会话 | 两端安装 Removent、完成配对、主机可达 |
| VNC / Apple 远程桌面 | 已有 VNC 或 macOS 屏幕共享 | 受支持的服务端及其凭据 |
| RDP | Windows 远程桌面 | 可达的 RDP 服务端及已授权账户 |

Removent 配对、原生音频、剪贴板和自适应媒体属于 RVP。兼容连接有各自的限制，详见 [VNC](/docs/vnc) 和 [RDP](/docs/rdp)。

## 跨网络连接

设备发现用于局域网。要访问其他网络中的 Mac，请配置[私有中继](/docs/relay)。你可以在 VPS 或 Cloudflare Containers 上运行中继，无需 Removent 托管账户。

## 准备无人值守的 Mac

在被控端有人操作时完成配对与隐私权限设置，然后配置[后台服务](/docs/hosting)。托管需要已登录的图形会话，不提供 FileVault 启动前访问能力。
