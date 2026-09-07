# 发版渠道与自动更新（GitHub Releases 开源路线）

历史设计记录（部分描述已过时）：当前实现与发布命令以 [发布指南](../docs/release-pipeline.md) 为准。

版本：v1.0（2026-08-22） · 上游：[architecture.md §9](./architecture.md)

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
 → zip / dmg 打包，生成 latest.json（scripts/gen_latest.py）
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
- 启动后 30s 与每 24h 各查一次 `GET <releases>/latest/download/latest.json`（用户可在设置关闭；企业内网屏蔽时不影响主功能）。
- 有新版：设置页与菜单栏出现角标提示，**仅提示不强更**。

### 3.2 安装流程（状态机）
```
Idle → Downloading(到 userdata/cache/update/Removent-<ver>-macos-arm64.zip, 断点续传)
     → Verifying(sha256 == 清单 && Ed25519 验签(载荷为上述四行) && 解包后 codesign --verify 有效且 TeamID 匹配)
     → Swapping(mv 旧 .app→app.old; mv 新 .app 就位)
     → Relaunch(spawn 新二进制 + 自身退出; 启动成功后清理 app.old, 失败则回滚)
```
要点：
- 校验三重：清单哈希、发布签名、Apple codesign。任一失败即丢弃下载并告警，绝不执行。
- 更新期间会话进行中则推迟到会话结束。
- 版本兼容：若 `min_compatible_proto` 高于本端，提示"两端需一起升级"，避免升级一端导致无法互连。

## 4. 发布纪律

1. 当前维护者流程：直接提交到 main，完成 CI 和发布验收后，从 main 创建不可变版本 tag 发布；不要求开 PR。
2. 每个 release 附 changelog（keepachangelog 格式），标注协议版本变更。
3. 回滚预案：Releases 保留全部历史版本可手动重下；更新器内置"恢复上一版"入口（利用 app.old）。
