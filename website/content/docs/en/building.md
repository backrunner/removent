---
title: "Build from source"
description: "Run Removent locally with Rust, Xcode, and the native development scripts."
order: 10
---

## Prerequisites

Use an Apple silicon Mac with macOS 13 or later, a stable Rust toolchain (1.89+), and full Xcode with Swift and the Metal toolchain.

## Clone and run

```sh
git clone https://github.com/backrunner/removent.git
cd removent
scripts/dev.sh
```

This builds incrementally and starts the desktop app, daemon, and menu bar app. The first build compiles dependencies; later builds reuse them.

## Development options

```sh
scripts/dev.sh --no-build
scripts/dev.sh --no-tray
scripts/dev.sh --build-only
scripts/dev.sh --help
```

Development uses `userdata/dev` unless `REMOVENT_DATA_DIR` is set. The script skips scheduled update checks and stops its own processes on Control+C. Keep this data separate from your installed application.

## Package locally

```sh
scripts/package.sh
```

The script produces an app bundle and zip under `dist/`, ad-hoc signed for local development. Public releases require the signing and notarization workflow described in the repository's [release guide](https://github.com/backrunner/removent/blob/main/docs/release-pipeline.md).

## Validate changes

```sh
cargo test --workspace
bash tray/Tests/run_integration_test.sh
```

Automated checks do not replace testing screen capture, macOS permissions, or real connections on target hardware.

## Work on this website

From `website/`, use Node 24+ and pnpm:

```sh
pnpm install
pnpm dev
pnpm check
pnpm check:docs
pnpm build
```

The website uses svedocs and SvelteKit with static output. Its bilingual public content lives under `website/content/`.
