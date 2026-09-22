---
title: "VNC 与屏幕共享"
description: "使用 Removent 连接 VNC 服务和 macOS 屏幕共享。"
order: 4
---

## 作为查看器连接

选择**添加连接 → VNC / Apple 远程桌面**。输入主机名或 IP、端口（默认 `5900`）及服务端凭据。支持 IPv4、IPv6 和自定义端口。

标准 VNC 服务通常只需密码。Apple 远程桌面或 macOS 屏幕共享使用 macOS 用户名与密码。

## 支持的身份验证

Removent 可协商 RFB 3.3、3.7 和 3.8，支持：

- 无身份验证（类型 1）。
- VNC 密码（类型 2）。
- Apple ARD Diffie-Hellman/AES（类型 30）。

尚未实现 Apple 类型 35、VeNCrypt/TLS、RealVNC 专有认证和 UltraVNC MS-Logon。服务端需要提供受支持的方法。

## 显示与输入

查看器支持 Raw、CopyRect、Hextile 和桌面尺寸变化。键盘、点击和滚动保持顺序；连续指针移动可以合并为最新位置。

使用 **Control + Command + I** 查看接收速率、像素处理和输入队列信息。本地输入发送耗时不包含远端响应时间。

## 通过 VNC 共享 Removent

Removent 被控端可选启用 VNC 监听器，默认关闭。在设置中启用 **Apple 远程桌面 / VNC**，设置密码，然后重启主机服务。

空密码选择无认证 RFB，仅适用于隔离且可信的局域网。VNC 不继承 Removent 原生双向 TLS 或配对机制，请使用可信网络或单独加密的隧道。

## 兼容性边界

VNC 路径提供帧缓冲显示与键鼠输入。原生 Removent 配对、音频、剪贴板和自适应媒体通过 [RVP](/docs/connecting) 提供，不属于此兼容服务。
