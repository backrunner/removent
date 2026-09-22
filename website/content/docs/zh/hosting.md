---
title: "后台托管"
description: "让主机服务独立于桌面和菜单栏应用持续运行。"
order: 6
---

## 三个独立进程

**Removent.app** 提供控制端与桌面界面；**removentd** 负责托管、捕获、设备身份和输入；**RemoventTray** 提供菜单栏控制、配对 PIN 和准入提示。

退出主窗口或菜单栏应用不会停止安装版的后台服务。关闭共享是另一项会持久保存的操作。

## 准备无人值守访问

1. 完成 [daemon 权限设置](/docs/permissions)。
2. 在被控端有人时，为每个授权控制端完成配对。
3. 启用**登录时启动服务器**。
4. 按需启用**登录时显示菜单栏应用**，这是独立设置。
5. 根据使用需求调整系统睡眠设置。
6. 关闭桌面与菜单栏应用，在实际主机上测试重连以及退出登录后重新登录。

受信任设备可在已批准的范围内重连，未知设备和新增能力仍需授权。

## 使用 CLI 管理

```sh
REMOVENT_CLI=/Applications/Removent.app/Contents/MacOS/removent-cli
"$REMOVENT_CLI" daemon start
"$REMOVENT_CLI" daemon enable
"$REMOVENT_CLI" daemon status
"$REMOVENT_CLI" daemon login-on
"$REMOVENT_CLI" daemon service-status
```

`daemon login-off` 移除之后的登录注册，不停止当前服务。`daemon disable` 持久关闭共享，保留管理入口。`daemon stop` 立即停止进程，不改变登录偏好。

重启或停止 daemon 会中断活动会话。

## 可用性边界

主机以用户级 LaunchAgent 运行，需要已登录的图形会话。不提供 FileVault 启动前、初始登录窗口、已退出用户或睡眠中 Mac 的访问。锁屏及无显示器行为需要在目标 Mac 和 macOS 版本上验证。

后台登录注册本身不能授予隐私权限、解锁或唤醒 Mac。

## 数据与移除

安装版数据位于 `~/Library/Application Support/removent/userdata`，其中包含 `logs/` 日志目录。`REMOVENT_DATA_DIR` 可覆盖数据位置。

移除后台运行前，关闭菜单栏登录项，运行 `daemon login-off`，再运行 `daemon stop`。删除应用不会删除设备身份或设置。
