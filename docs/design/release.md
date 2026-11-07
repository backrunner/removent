# 发版渠道与自动更新（GitHub Releases 开源路线）

历史设计记录（部分描述已过时）：当前实现与发布命令以 [发布指南](../guides/release-pipeline.md) 为准。

版本：v1.0（2026-08-22） · 上游：[architecture.md §9](architecture.md)

## 1. 渠道与产物

唯一官方渠道：**GitHub Releases**（`github.com/<org>/removent`）。每个 tag 触发流水线产出：

| 产物 | 说明 |
| --- | --- |
| `Removent-<ver>-macos-arm64.zip` | 主安装物，也是自动更新的下载对象（ditto 打包，解压即用，便携 userdata 设计） |
| `Removent-<ver>-macos-arm64.dmg` | 拖拽安装镜像（hdiutil UDZO，公证 + staple） |
| `latest.json` | 更新元数据：version / url / sha256 / notes / pub_date / min_compatible_proto / signature |
| ~~`SHA256SUMS` + `.sig`~~ | 未实现；完整性由 latest.json 的 sha256 + Ed25519 签名覆盖 |

命名与 SemVer：`vMAJOR.MINOR.PATCH`；`PROTO_VERSION` 随 MAJOR 演进（protocol.md §8）。

## 2. 构建签名公证流水线（GitHub Actions）

```
tag push v*
 → macos-latest runner: cargo build --release (aarch64-apple-darwin)
 → 组装 .app（Info.plist 含 NSScreenCaptureUsageDescription、hardened runtime entitlements）
 → codesign --sign "Developer ID Application" --options runtime
 → xcrun notarytool submit（App 专属密码/APP API key，存 GitHub Secrets）
 → xcrun stapler staple
 → zip / dmg 打包，生成 latest.json（scripts/release/gen_latest.py）
 → Ed25519 签名 latest.json：签名载荷为 UTF-8 的 `"{version}\n{url}\n{sha256}\n{min_compatible_proto}"`
   （四行、无尾换行），用 `openssl pkeyutl -sign -rawin` 签名，hex 编码写入 signature 字段。
   私钥在 CI 来自 Secret `UPDATE_SIGNING_KEY`（PEM 内容），本地发布用
   ~/.config/removent/update-signing-key.pem；公钥（hex）编译进客户端。
 → gh release create 直接发布（非 draft；见下方说明）
```

- 工具选型：打包脚本自研（shell + cargo-dist 仅作参考）；CI 的签名密钥只存 Actions Secrets（`UPDATE_SIGNING_KEY`，运行时写入临时文件、用后删除），本地发布从维护者本机 `~/.config/removent/update-signing-key.pem` 读取。
- 发布不经 draft：workflow 用 `gh release create` 直接发布（`.github/workflows/release.yml`）。刻意不要 draft 化——draft 期间 `releases/latest/download/latest.json` 返回 404，所有客户端的自动更新检查会暂时失败（手动检查会报"获取更新信息失败"）。
- 双许可声明 MIT OR Apache-2.0 写入 Cargo.toml `license` 与仓库 LICENSE 文件对。

## 3. 客户端自动更新

### 3.1 检查策略
- 0.1.12 起自动安装默认开启，允许关闭；已有 `update_check_enabled = false` 不被覆盖。
- 桌面端 30 秒及每 24 小时检查，独立用户更新器每 6 小时检查。窗口关闭且连接结束后安装，不打断现有会话。
- Stable/Beta、签名清单、版本和协议约束统一由 `packages/updater` 实现。

### 3.2 安装与恢复

`检查 → 下载并验签 → 权限及真实主程序启动预检 → 持久事务 → 暂停 host → 同盘替换 → host 连续健康检查 → 保留备份`

失败从已验证的 `.app.update-backup` 恢复。安装进程中断时，独立代理先恢复事务，再读取更新设置或访问网络。安装前及安装中以共享/排他准入锁保护会话，持久维护标记覆盖进程死亡。主动停止的 host 不被更新流程重新开启。

更新器使用应用目录外的固定监护进程及可替换的签名工作进程；新版工作进程故障时回到旧副本。登录前 host 使用独立 root-owned 副本，保持原有服务并提示管理员更新，不越权静默替换。

详细契约、权限边界及验收命令见[自动更新](../guides/automatic-updates.md)。

## 4. 发布纪律

1. 当前维护者流程：直接提交到 main，完成 CI 和发布验收后，从 main 创建不可变版本 tag 发布；不要求开 PR。
2. 每个 release 附 changelog（keepachangelog 格式），标注协议版本变更。
3. 回滚预案：Releases 保留全部历史版本可手动重下；独立更新器执行经过健康检查的自动回退，并保留 `.app.update-backup` 供恢复。
