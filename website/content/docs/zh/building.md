---
title: "从源码构建"
description: "使用 Rust、Xcode 和原生开发脚本在本机构建 Removent。"
order: 10
---

## 准备工具

使用运行 macOS 13 或更新版本的 Apple 芯片 Mac、稳定版 Rust 工具链（1.89+），以及包含 Swift 和 Metal 工具链的完整 Xcode。

## 克隆并启动

```sh
git clone https://github.com/backrunner/removent.git
cd removent
scripts/dev.sh
```

脚本增量构建并启动桌面应用、daemon 和菜单栏应用。首次构建需要编译依赖，之后会复用构建产物。

## 开发选项

```sh
scripts/dev.sh --no-build
scripts/dev.sh --no-tray
scripts/dev.sh --build-only
scripts/dev.sh --help
```

开发默认使用 `userdata/dev`，可以通过 `REMOVENT_DATA_DIR` 覆盖。脚本跳过计划更新检查，在 Control+C 时停止自己启动的进程。请将开发数据与正式安装的数据分开。

## 本地打包

```sh
scripts/package.sh
```

脚本在 `dist/` 生成应用包与 zip，使用临时签名供本地开发。公开发布需使用仓库[发布指南](https://github.com/backrunner/removent/blob/main/docs/release-pipeline.md)中的签名和公证流程。

## 验证改动

```sh
cargo test --workspace
bash tray/Tests/run_integration_test.sh
```

自动化检查不替代目标硬件上的屏幕捕获、macOS 权限和真实连接测试。

## 开发本网站

在 `website/` 目录中使用 Node 24+ 与 pnpm：

```sh
pnpm install
pnpm dev
pnpm check
pnpm check:docs
pnpm build
```

网站基于 svedocs 与 SvelteKit，输出静态文件。中英文公开内容位于 `website/content/`。
