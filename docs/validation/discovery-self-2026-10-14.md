# 0.1.5 局域网发现排除本机

用户截图中，本机 VNC 服务 `Mac Studio`（`127.0.0.1:5900`）出现在局域网发现列表。桌面原先仅按 Removent 指纹排除本机，VNC/RDP 没有地址排除；移动端直接选用 Bonjour 返回的地址。

## 修复

- `LocalDiscoveryAddresses` 从系统 `getifaddrs` 读取实际分配的 IPv4/IPv6 地址，不执行 DNS 查询、端口扫描或连接探测。
- 桌面端和移动端的三个协议共用这一策略。移动端通过独立 C ABI 调用同一 Rust 实现，不创建引擎或读取凭据。
- 排除回环、未指定、不可用的地址以及本机网卡地址，规范化 IPv4-mapped IPv6，保留 IPv6 link-local 的接口 scope。
- 如果广播中的有效地址属于本机，则整个记录被隐藏；远端记录夹带回环别名时，选用其有效远端地址。同名设备不作为排除依据。
- 两端每两秒重新检查系统接口，网络变化后移除旧的本机条目。未取得有效接口快照时暂不展示未确认的发现条目。
- 此过滤应用于自动发现列表。显式连接、书签和输入邀请连接码的解析继续使用连接路径。

## 验证

| 检查 | 结果 |
| --- | --- |
| 修改前的回环发现回归 | 失败，证实缺少回环过滤 |
| 完整 Rust 工作区 | 472 passed，0 failed，7 ignored |
| 最终桌面发现相关测试 | 10 passed，0 failed |
| 实际 mDNS multicast 发现测试 | 14 passed，0 failed；实际本机 Removent/VNC/RDP 广播被显示策略排除 |
| 实际网卡快照与地址边界 | IPv4、IPv6、mapped IPv4、link-local scope、同名远端、别名及地址变化均通过 |
| Strict Clippy、格式、布局检查 | 通过 |
| iOS 单元测试 | 21 passed，0 failed；经 C ABI 逐一验证系统实际网卡地址被排除 |
| iOS 邀请连接 UI | 通过真实 RVP 视频、输入、邀请消耗及记住主机后重连 |
| iOS arm64 Release App | 未签名构建及完整链接通过 |
| 官网检查、文档、构建与浏览器 | 32 个 Chromium/WebKit 场景通过 |
| 发布脚本与 iOS 元数据 | 通过 |

主要日志位于 `/tmp/removent-015-reproduce.log`、`/tmp/removent-015-workspace.log`、`/tmp/removent-015-desktop-final.log`、`/tmp/removent-015-multicast-final.log`、`/tmp/removent-015-ios-final.log` 与 `/tmp/removent-015-ios-invitation.log`。iOS 结果保存于 `target/mobile-stable-015/Logs/Test/*.xcresult`。

本次验证使用 macOS 实际接口快照、真实本地 mDNS 和 iOS Simulator。未在物理 iPhone、真实 Wi-Fi 切换或用户当前安装的桌面窗口中验收，不改变正在进行的用户会话。远端发布验收另以正式 tag 的同一提交、CI/Relay/Release 结果及公开产物校验为准。
