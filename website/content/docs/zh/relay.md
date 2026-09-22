---
title: "私有中继"
description: "一键安装预编译的私有中继，使用原生命令行管理。"
order: 7
---

中继在不同网络间承载加密的原生 RVP 流量，设备配对、能力授权与双向 TLS 保持端到端。中继能看到端点 IP、房间名、流量时序与大小，但不解码桌面画面。

## 在 VPS 上安装

使用 Linux x86_64 或 ARM64，配备 systemd 247 或更新版本，例如 Ubuntu 22.04+、Debian 12+。服务器需要 curl 和 tar，无需安装 Rust 工具链或 Docker。

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

安装器下载预编译二进制，校验 SHA-256 和版本，然后打开原生设置向导。填写公开地址（例如 `removent://relay.example.com:48700`）、房间与可选来源网段，程序会生成独立的被控端和控制端凭据，并启动非特权 systemd 服务。请在 VPS 防火墙放行所选 UDP 端口。

选用的正式 GitHub Release 必须包含 relay 二进制资产。尚未提供这些资产的旧版本无法通过此命令安装。

如需先检查脚本或指定版本安装：

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh -o install_relay.sh
sh install_relay.sh --version v0.1.1 --no-setup
sudo removent-relay setup --address removent://relay.example.com:48700
```

## 在 Mac 上托管中继

同一条 curl 命令也可在 **macOS 13+** 上安装完整的 relay，Universal 二进制同时支持 **Apple Silicon 和 Intel**。请以普通登录用户运行安装器，只有写入 `/usr/local/bin` 时需要 sudo，配置向导和日常服务管理都无需 sudo。

```sh
removent-relay status
removent-relay start
removent-relay stop
removent-relay restart
removent-relay logs --follow
removent-relay enable
removent-relay disable
```

当前用户的 LaunchAgent 会在登录时启动，并在意外退出后恢复。`enable / disable` 只影响下次登录，不中断正在运行的中继；`stop` 卸载当前作业，避免 launchd 立即重新拉起。退出登录后 relay 会停止，Mac 睡眠期间无法转发流量。常驻使用时需保持用户登录和机器唤醒，并按需配置防火墙与路由器 UDP 转发。

私密配置、连接文件、身份与日志位于 `~/Library/Application Support/removent-relay`。它与桌面 App 的后台 daemon 独立，不需要屏幕录制或辅助功能权限。macOS 可直接导出到当前用户目录：

```sh
removent-relay check
removent-relay fingerprint
removent-relay export host --output "$HOME/relay-host.toml"
removent-relay export client --output "$HOME/relay-client.toml" --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

升级时再次运行安装器，再执行 `removent-relay restart`。安装保留凭据、证书、登录自启偏好和当前停止状态。`uninstall` 移除 LaunchAgent，保留数据及二进制。这里的 Intel 支持适用于独立 relay，桌面 App 仍遵循其安装要求。

## 管理 Linux 服务

```sh
sudo removent-relay start
removent-relay status
sudo removent-relay logs --follow
sudo removent-relay stop
sudo removent-relay restart
```

`enable` 和 `disable` 单独控制开机启动。停止服务不会取消下次开机启动。`check` 校验配置，`fingerprint` 输出已有中继证书指纹；读取私密配置或修改服务时使用 sudo。

再次运行安装器即可升级，然后显式执行 `restart` 使用新二进制。升级保留配置、凭据和身份，重启会断开现有连接。`uninstall` 移除系统服务，但保留上述文件与二进制。

## 连接两端 Mac

Linux 设置向导在 `/etc/removent-relay` 中生成私密连接文件，macOS 使用上一节的用户目录，两份文件都已固定中继证书指纹。通过 CLI 导出：

```sh
sudo removent-relay export host --output /root/relay-host.toml
sudo removent-relay export client --output /root/relay-client.toml --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

将 `VERIFIED_TARGET_MAC_FINGERPRINT` 替换为被控 Mac 上 `removent-cli identity` 输出的完整指纹，并通过可信渠道核对。

1. 将 host 文件安全传输到被控 Mac 的 daemon 数据目录，可从托盘的**打开数据目录**进入。文件须属于 Mac 当前用户，权限为 0600，然后重启后台服务。
2. 控制端选择**添加连接 → Removent → 通过中继连接 → VPS / QUIC**。
3. 从已验证的 client 配置填写地址、房间、控制端凭据、中继指纹与目标 Mac 指纹。
4. 在无人值守前完成设备配对和访问授权。

两种角色的凭据独立，只分发对应连接文件。桌面端将中继凭据存入 macOS 钥匙串。Linux 请私密备份 `/etc/removent-relay` 与 `/var/lib/removent-relay` 的实际内容，macOS 备份完整的 relay 用户目录；重新生成中继身份后不可继续沿用旧指纹。

## Cloudflare Containers

Cloudflare 使用 443 端口的 HTTPS / WebSocket。首次部署基础设施需要已开通 Containers 的账户、Node 24+、Python 3.11+ 和可构建 Linux amd64 的 Docker。Worker 和容器设置见 [Cloudflare 部署指南](https://github.com/backrunner/removent/blob/main/docs/cloudflare-relay.md)。

Linux 和 macOS 使用同一个 CLI；仅管理 Cloudflare、不在本机托管 relay 时，给安装器传入 `--no-setup`。部署完成后，直接管理 Cloudflare：

```sh
removent-relay cloudflare start
removent-relay cloudflare status
removent-relay cloudflare stop
```

CLI 从 `~/.config/removent-relay`（或 `$XDG_CONFIG_HOME/removent-relay`）读取上次成功部署的地址和私密管理凭据，其他部署使用 `--config-dir` 指定目录。手动部署也可以传入 `--url https://YOUR_WORKER --credential-file /PRIVATE/PATH/admin.token`，凭据保存在私密文件中，不放入命令参数。

手动停止状态会持久保存，被控端重试不能启动已停止的容器。已启用的容器可在空闲时休眠，并在控制端访问时唤醒。容器日志在 Cloudflare 控制台查看，基础设施费用由托管提供商决定。

## 配置参考

[安装与配置指南](https://github.com/backrunner/removent/blob/main/docs/relay-quick-deploy.md)涵盖来源白名单、无人值守安装、配置文件、升级和独立前台运行。协议与信任边界见 [VPS 参考](https://github.com/backrunner/removent/blob/main/docs/private-relay.md)。
