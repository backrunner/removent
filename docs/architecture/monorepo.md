# Monorepo layout

The root Cargo workspace owns Rust versions, dependency aliases, patches and build
profiles. Swift and web targets keep their native manifests and lockfiles; `make`
provides repository-wide entry points without introducing another package manager.

| Directory | Responsibility |
| --- | --- |
| `apps/desktop` | GPUI desktop UI and platform orchestration |
| `apps/cli`, `apps/daemon`, `apps/relay` | Executable entry points and process/service management |
| `apps/mobile`, `apps/tray`, `apps/cloud-sync-helper`, `apps/installer` | Apple native application targets |
| `apps/website`, `apps/relay-worker` | Public documentation website and Cloudflare relay controller |
| `packages/core`, `packages/proto`, `packages/net` | Shared configuration, identity, protocol and networking |
| `packages/client`, `packages/host` | Reusable controller and hosting engines |
| `packages/media-capture`, `packages/media-codec`, `packages/input` | Capture, codecs and input adapters |
| `packages/relay-transport` | Shared QUIC/WebSocket relay library used by clients, hosts and the relay application |
| `packages/mobile` | Rust controller ABI consumed by the native mobile app |
| `packages/apple-cloud-sync` | One Swift CloudKit implementation shared by mobile and the macOS helper |
| `infra/relay`, `infra/macos` | Deployment templates and service definitions |
| `scripts/build`, `scripts/release`, `scripts/relay`, `scripts/macos` | Build, publication, relay operations and system installation tools, with colocated offline tests |
| `scripts/qa`, `scripts/fixtures` | Opt-in acceptance runners and synthetic fixtures |
| `assets/branding` | Shared brand assets |
| `docs/design`, `docs/architecture`, `docs/guides`, `docs/validation`, `docs/releases` | Original design records, current architecture, operating guides, validation evidence and release notes |
| `vendor` | Explicitly patched third-party dependencies |
| `target`, `dist`, `userdata` | Ignored build output, distributable artifacts and private runtime data |

## Ownership rules

Application-specific UI, lifecycle and service commands belong in `apps/`.
Reusable implementation belongs in `packages/`. A package's normal or build
dependencies must not point into an application. Integration tests and examples
stay beside the implementation they exercise. The existing Rust package aliases,
binary names, wire formats and user data paths are preserved.

Large modules use a small `mod.rs` for state and public exports and named child
modules for each responsibility. Production Rust files have a 650-line budget;
test modules have a 1,000-line budget. Split at behavior boundaries and keep
internal visibility limited to the parent module. Test-only imports stay behind
`cfg(test)`. Run `python3 scripts/check_layout.py` to enforce these rules.

Swift controller extensions share internal state inside the tray executable;
the CloudKit transport lives in its own Swift package. XcodeGen specifications
are authoritative; regenerate ignored Xcode projects with `make mobile-projects`.

## Commands

| Command | Checks or action |
| --- | --- |
| `make check` | Layout, offline Python tests, shell syntax, Rust formatting and Clippy |
| `make test` | Rust workspace tests, including loopback protocol/media integration |
| `make apple-test`, `make tray-test` | Shared CloudKit unit tests and isolated tray IPC integration |
| `make website-check` | Website types, translated docs, static build and browser tests |
| `make relay-worker-check` | Cloudflare Worker types and tests |
| `make dev` | Development desktop, daemon and tray |
| `bash scripts/build/package.sh` | Development app bundle |
| `bash scripts/release/release.sh` | Signed release pipeline, requiring the existing signing configuration |

`scripts/dev.sh`, `scripts/install.sh` and `scripts/install_relay.sh` remain stable
top-level entry points. The installers are fetched individually by users and
therefore remain self-contained. Native live lifecycle and cloud acceptance
scripts stay opt-in under `scripts/qa/` and the relevant component scope.

## 中文说明

根目录的 Cargo workspace 统一管理 Rust 版本、依赖和构建配置；Swift 和网站保留各自原生
包管理方式，通过根目录 Makefile 提供统一入口。应用入口放在 `apps/`，公共能力放在
`packages/`，公共包的运行和构建依赖不得指向应用。部署模板放在 `infra/`，脚本按构建、
发布、中继运维、macOS 安装、验收和 fixture 分类；文档按设计、架构、指南、验证及发布分类。

大文件按职责拆成具名子模块，Rust 生产文件限制为 650 行，测试模块限制为 1,000 行；
目录检查会阻止依赖方向倒置和新的超大文件。现有二进制名、协议及用户数据位置保持一致。
Xcode 项目由 `project.yml` 生成并忽略，移动端和 macOS 同步助手引用同一份 CloudKit 源码。
