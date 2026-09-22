# Relay 一键安装与原生命令行

Relay 使用预编译 Rust 二进制，支持 Linux x86_64 / ARM64，以及 macOS 13+ 的 Apple Silicon / Intel。Linux 后台服务使用 systemd 247+（例如 Ubuntu 22.04+、Debian 12+），macOS 使用当前用户的 launchd LaunchAgent。目标机器不需要 Rust、Docker、Node 或 Python。

> 安装器使用从 v0.1.0 开始的正式 GitHub Release 资产。指定版本若没有对应资产，安装会明确失败，保留已有安装。

## Linux 安装

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

安装器解析最新稳定版本，下载固定版本的架构包及 SHA-256 文件，校验包内容与二进制版本，再原子替换 `/usr/local/bin/removent-relay`。首次安装由二进制交互询问公开地址（`removent://relay.example.com:48700`）、房间和来源网段，并创建、启用和启动 systemd 服务。需要写系统目录时会使用 sudo。

先检查脚本或无人值守安装：

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh -o install_relay.sh
sh install_relay.sh --version v0.1.0 --no-setup
sudo removent-relay setup --address removent://relay.example.com:48700 --room office
```

可以在安装命令的 `--` 后传入原生 setup 参数：

```sh
sh install_relay.sh -- --address removent://relay.example.com:48700 --allow-cidr 203.0.113.0/24,2001:db8::/32
```

放行 VPS 防火墙和云安全组的 UDP 48700（或自选端口）。安装器不修改防火墙。默认监听 IPv4；需要 IPv6 时给 setup 传入 `--listen '[::]:48700'`，并检查服务器双栈与防火墙设置。网段白名单填写设备访问中继时的公网出口；空列表允许所有来源，但仍强制身份认证。

## macOS 安装与托管

在已登录的 Mac 上用普通用户运行同一个 curl 安装命令，**不要用 sudo 运行整份脚本**。安装器自动选择 Universal 包，校验后安装到 `/usr/local/bin`，只有写入该目录时使用 sudo；设置向导和 relay 进程以当前用户身份运行。

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
removent-relay status
removent-relay logs --follow
removent-relay stop
removent-relay start
removent-relay restart
removent-relay enable
removent-relay disable
```

首次安装自动设置登录启动。`enable / disable` 只控制下次登录，不中断正在运行的会话；`stop` 会卸载当前 launchd 作业，避免被 KeepAlive 重新拉起。意外退出由 launchd 恢复。命令在端口绑定并收到该进程的就绪通知后才报告启动成功。

- 配置、连接文件、证书和日志位于 `~/Library/Application Support/removent-relay`，私密目录 0700，文件 0600。
- 登录启动文件为 `~/Library/LaunchAgents/com.alkinum.removent.relay.plist`，与桌面 App 的 daemon 是两个独立服务。
- 日志位于 `logs/relay.log` 和 `logs/relay.err.log`，停止后再次启动时超过 10 MiB 的日志保留为 `.1`。
- 退出用户登录会停止 relay；睡眠期间无法转发流量。作为常驻中继时，需保持该用户登录并配置合适的电源设置。跨网络访问还需公网可达地址、路由器 UDP 转发及防火墙允许。
- `removent-relay uninstall` 移除服务及登录启动项，保留配置、凭据、证书和二进制。

macOS 上的 `setup / check / fingerprint / export` 自动使用上述目录，均无需 sudo：

```sh
removent-relay setup --address removent://relay.example.com:48700
removent-relay check
removent-relay fingerprint
removent-relay export host --output "$HOME/relay-host.toml"
removent-relay export client --output "$HOME/relay-client.toml" --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

仅需 Cloudflare 管理 CLI 时，安装器可传 `--no-setup`。高级用法可用 `--bin-dir "$HOME/.local/bin" --service-dir /PRIVATE/PATH/relay` 安装一个独立实例；以后每个原生命令都加 `--service-dir /PRIVATE/PATH/relay`，独立实例不会控制默认服务。替代目录不会改变 `cloudflare` 子命令的部署目录。

macOS relay 的 Intel 支持适用于独立中继程序；桌面 App 的系统要求见其安装文档。

## Linux 日常管理

```sh
sudo removent-relay start
sudo removent-relay stop
sudo removent-relay restart
removent-relay status
sudo removent-relay logs --follow
sudo removent-relay enable
sudo removent-relay disable
sudo removent-relay check
sudo removent-relay fingerprint
```

`start / stop / restart` 控制当前运行状态，`enable / disable` 控制开机启动。停止不会取消开机启动；需要跨重启保持停止时，同时执行 `disable`。启动等到端口成功绑定才报告完成，服务以非特权动态用户运行。VPS 停止中继不会停止 VPS 计费。

重复安装只替换程序，保留配置、凭据和证书。升级后执行 `sudo removent-relay restart` 使用新二进制，会断开已有会话。指定 `--version vX.Y.Z` 可安装该版本，再显式重启。重复运行 `setup` 不会重新生成凭据；已有配置请直接编辑并执行 `check`、`restart`。

`sudo removent-relay uninstall` 停止并移除系统服务，保留二进制、私密配置和身份，方便恢复。重新运行 `setup` 可恢复服务。

## Linux 配置与连接文件

| 路径 | 内容 |
| --- | --- |
| `/etc/removent-relay/server.toml` | 服务端参数、来源白名单与角色凭据哈希 |
| `/etc/removent-relay/relay-host.toml` | 被控端配置和独立的 host 凭据 |
| `/etc/removent-relay/relay-client.toml` | 控制端配置和独立的 client 凭据 |
| `/var/lib/removent-relay` | 持久化中继证书与私钥，由 systemd 管理 |

配置目录 0700，配置和连接文件 0600。服务通过 systemd LoadCredential 读取配置。原始凭据不会打印在初始化输出中；两份连接文件已经固定中继证书指纹。备份配置和身份目录，并保持私密；动态用户的状态目录可能指向 `/var/lib/private/removent-relay`，备份时须包含实际内容。

导出连接文件，不会覆盖已有文件：

```sh
sudo removent-relay export host --output /root/relay-host.toml
sudo removent-relay export client --output /root/relay-client.toml --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

`VERIFIED_TARGET_MAC_FINGERPRINT` 替换为被控 Mac 上 `removent-cli identity` 输出的完整指纹，通过可信渠道核对。将 host 文件安全传输到被控 Mac 的 daemon 数据目录（托盘的“打开数据目录”），文件归 Mac 当前用户所有、权限 0600，再重启后台服务。client 文件只分发给控制端，不要互换角色凭据。

控制端可导入其中参数到“添加连接 → Removent → 通过中继连接”，或运行：

```sh
removent-cli ping --relay-profile /PRIVATE/PATH/relay-client.toml
```

中继身份与目标 Mac 身份是独立的，仍需完成 RVP 配对和能力授权。修改服务端凭据哈希后，重新制作相应连接文件；`export` 会拒绝与当前服务端不匹配的旧凭据。

不使用 systemd 时，可仅安装二进制，再用 `removent-relay init --dir /PRIVATE/PATH/relay --address removent://relay.example.com:48700` 生成配置，使用 `removent-relay serve /PRIVATE/PATH/relay/server.toml` 前台运行。`removent-relay --help` 提供完整命令。

## Cloudflare Containers

Cloudflare 部署需要创建 Worker / Durable Object / Container 基础设施，仍使用仓库中的部署工具；日常启停使用已安装的原生 CLI。

首次部署在管理电脑安装 Python 3.11+、Node 24+ 和可构建 Linux amd64 的 Docker。账户须开通 Containers。在 `deploy/cloudflare` 中执行 `npm ci`、`npx wrangler login`，然后在仓库根目录运行 `bash scripts/deploy_relay.sh` 并选择 Cloudflare。这一步生成独立的 host / client / admin 凭据，部署完成后保持停止。

```sh
removent-relay cloudflare start
removent-relay cloudflare status
removent-relay cloudflare stop
removent-relay cloudflare status --config-dir "$HOME/.config/removent-office-relay"
```

CLI 默认使用 `$XDG_CONFIG_HOME/removent-relay`（否则 `~/.config/removent-relay`）中上次成功部署的 `deployed-origin` 和 `deployed-admin.token`，未部署的凭据编辑不会破坏管理入口。也可显式传入 `--url https://YOUR_WORKER --credential-file /PRIVATE/PATH/admin.token`；凭据文件必须属于当前用户且为 0600，不接受把 token 放入命令行。

部署配置目录为 0700、私密文件 0600；`deployment.json` 管理地址、房间、资源限制、休眠和网段，`credentials.json` 保存各角色凭据。修改后执行 `bash scripts/deploy_relay.sh deploy` 应用配置。不要直接编辑自动生成的 `runtime/*`。更换 backend、部署名或地址需要使用新的配置目录。示例见 [deployment.example.json](../deploy/relay/deployment.example.json)。

`allowed_cidrs` 限制连接来源，`admin_allowed_cidrs` 为管理入口独立白名单（`null` 继承，`[]` 不限制）；确保管理电脑的出口被允许。最多 256 个 IPv4/IPv6 网段，单个地址使用 `/32` 或 `/128`。Cloudflare Pseudo IPv4 使用 Off 或 Add Header。

管理员停止状态持久化；被控端重试不能唤醒已停止容器。已启用容器默认在无控制端 300 秒后休眠，控制端连接可唤醒。日志在 Cloudflare 控制台查看。完整配置、安全边界和生命周期见 [Cloudflare 部署说明](cloudflare-relay.md)。

## 发布与验证

正式发布流水线在 Linux x86_64 / ARM64 原生 runner 构建静态 musl 二进制，运行 relay 测试及真实 systemd 安装、就绪、启停和身份保留验收；macOS 将 ARM64 / x86_64 合并为 Universal 二进制，完成 Developer ID 签名、公证和真实 launchd 生命周期验收。三个平台包及各自的 `.sha256` 文件随同一稳定 tag 发布，缺少任一平台就不会进入发布步骤。

本地测试覆盖配置权限、凭据隔离、拒绝覆盖、Cloudflare HTTP 方法和重定向拒绝，以及安装包校验失败时保留旧安装。macOS 使用独立标签、配置和端口实测 launchd 及安装器，不修改已有服务；登录启动、崩溃恢复、错误端口拒绝、重复安装保留停止状态均属于验收。真实 Linux systemd 验收脚本只应在可丢弃 runner 上运行，不能在已有 relay 上执行。Cloudflare 的真实部署和公网性能仍需目标环境验证。
