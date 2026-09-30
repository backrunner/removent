#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
platform=iphonesimulator
configuration=Debug
while [ "$#" -gt 0 ]; do
  case "$1" in
    --platform) platform="$2"; shift 2 ;;
    --configuration) configuration="$2"; shift 2 ;;
    *) echo "Usage: $0 [--platform iphoneos|iphonesimulator] [--configuration Debug|Release]" >&2; exit 2 ;;
  esac
done
case "$platform" in
  iphoneos) target=aarch64-apple-ios ;;
  iphonesimulator) target=aarch64-apple-ios-sim ;;
  *) echo "Unsupported platform: $platform" >&2; exit 2 ;;
esac
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:/usr/bin:/bin:$PATH"
# Select rustc explicitly: a Homebrew cargo/rustc can otherwise shadow rustup's
# installed iOS standard libraries, including when invoked by Xcode.
export RUSTC="$(rustup which --toolchain stable rustc)"
export RUSTDOC="$(rustup which --toolchain stable rustdoc)"
export IPHONEOS_DEPLOYMENT_TARGET=26.0
export SDKROOT="$(xcrun --sdk "$platform" --show-sdk-path)"
export CARGO_TARGET_DIR="$ROOT/target/mobile"
# Cross-compile bundled Opus instead of finding a macOS Homebrew dylib.
export OPUS_NO_PKG_CONFIG=1
export CMAKE_POLICY_VERSION_MINIMUM=3.5
export CMAKE_GENERATOR="Unix Makefiles"
profile=debug
args=()
if [ "$configuration" = Release ]; then profile=release; args+=(--release); fi
cd "$ROOT"
rustup run stable cargo build --locked -p removent-mobile --lib --target "$target" ${args[@]+"${args[@]}"}
mkdir -p "$ROOT/apps/mobile/Libraries/$platform"
cp "$CARGO_TARGET_DIR/$target/$profile/libremovent_mobile.a" "$ROOT/apps/mobile/Libraries/$platform/"
