---
title: "私有中继"
description: "在 Linux、macOS 或 Cloudflare 上部署私有中继，配置更新与本地日志。"
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
sh install_relay.sh --version v0.1.2 --no-setup
sudo removent-relay setup --address removent://relay.example.com:48700
```

## 试用 beta

自动更新、`check-update` 和 relay 本地日志轮转从 **v0.1.3-beta.1** 开始提供。安装器默认选择正式版，试用这些功能时需要显式选择 beta：

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/v0.1.3-beta.1/scripts/install_relay.sh | sh -s -- --version v0.1.3-beta.1
```

[查看 beta 版本说明和下载](https://github.com/backrunner/removent/releases/tag/v0.1.3-beta.1)。已有服务安装后需要重启。后续 beta 也须手动安装；自动更新只跟随正式版，包括从 beta 升级到更高版本的正式版。

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

## 更新与本地日志

### 开启或关闭自动更新

Relay 可自动安装带发布签名的正式版本，默认关闭。编辑 Linux 的 `/etc/removent-relay/server.toml` 或 macOS 的 `~/Library/Application Support/removent-relay/server.toml`：

```toml
[updates]
enabled = true
check_interval_secs = 86400
```

执行 `sudo removent-relay restart`（Linux）或 `removent-relay restart`（macOS）应用配置。开启后在启动 30 秒时检查，之后按配置间隔检查；间隔允许 3600–604800 秒。关闭时把 `enabled` 改为 `false`，再重启。

下载会校验 Ed25519 发布签名及归档和二进制的 SHA-256，macOS 还会校验 Developer ID。验证通过后使用新版检查现有配置，再重启 relay，短暂中断连接。检查、下载或验证失败时继续运行旧版。

### 手动检查与版本选择

手动检查不受自动更新开关影响，不下载二进制，也不安装：

```sh
# macOS 默认服务
removent-relay check-update
# Linux：读取私密配置和服务已安装的版本
sudo removent-relay check-update --config /etc/removent-relay/server.toml
# 其他实例
removent-relay check-update --config /path/to/server.toml
```

默认读取可访问的服务配置；没有可读配置时比较 CLI 自身版本。正式更新源必须提供 `relay-latest-<platform>.json` 签名清单。v0.1.2 没有这些清单；在更新源提供清单前，检查会报告获取失败，不影响现有转发。

更新保存在 `identity_dir/updates`，系统 CLI 保留为启动器，无需给后台进程系统目录写权限。关闭自动更新保留已经安装的新版、配置、凭据和身份。需要回退时，先停止服务并关闭自动更新，备份后移走 `identity_dir/updates`，安装指定版本，再启动服务。

### 日志位置与保留

Relay 主日志是 `identity_dir/logs/removent-relay.log`，默认位置如下：

| 组件 | 日志文件 |
| --- | --- |
| Linux relay | `/var/lib/removent-relay/logs/removent-relay.log` |
| macOS relay | `~/Library/Application Support/removent-relay/data/logs/removent-relay.log` |
| 桌面客户端、daemon、客户端 CLI | 数据目录下的 `logs/removent.log`、`logs/removentd.log`、`logs/removent-cli.log` |

每个文件最多 8 MiB，保留 `.log.1`–`.log.3` 三份备份，每个组件合计约 32 MiB；日志文件权限为 0600。崩溃报告另行保留最近 10 份。桌面数据目录可从托盘的**打开数据目录**进入，也可用 `REMOVENT_DATA_DIR` 覆盖。

`removent-relay logs --follow` 在 macOS 跟踪本地 relay 日志；Linux 的同名命令查看 systemd journal，本地轮转文件同时保留。Linux 可用 `sudo tail -F /var/lib/removent-relay/logs/removent-relay.log` 跟踪文件。

## Cloudflare Containers

Cloudflare 使用 443 端口的 HTTPS / WebSocket。首次部署基础设施需要已开通 Containers 的账户、Node 24+、Python 3.11+ 和可构建 Linux amd64 的 Docker。Worker 和容器设置见 [Cloudflare 部署指南](https://github.com/backrunner/removent/blob/main/docs/cloudflare-relay.md)。

Cloudflare 部署、启停和升级使用仓库脚本及管理 API，基础设施和日志可在 Cloudflare 控制台查看；不需要安装原生 relay CLI。在源码目录中运行：

```sh
bash scripts/deploy_relay.sh start
bash scripts/deploy_relay.sh status
bash scripts/deploy_relay.sh stop
bash scripts/deploy_relay.sh deploy
```

`deploy` 重新构建和部署容器，完成后保持停止，需要显式 `start`。容器不运行 relay 自动更新器；磁盘为临时存储，日志保留交给 Cloudflare。脚本或管理 API 的手动停止状态会持久保存，被控端重试不能启动已停止的容器。已启用的容器可在空闲时休眠，并在控制端访问时唤醒。容器日志在 Cloudflare 控制台查看，基础设施费用由托管提供商决定。

## 配置参考

[安装与配置指南](https://github.com/backrunner/removent/blob/main/docs/relay-quick-deploy.md)涵盖来源白名单、无人值守安装、配置文件、升级和独立前台运行。协议与信任边界见 [VPS 参考](https://github.com/backrunner/removent/blob/main/docs/private-relay.md)。
