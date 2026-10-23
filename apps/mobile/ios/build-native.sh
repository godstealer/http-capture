#!/bin/sh
set -eu

ios_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
workspace=$(CDPATH= cd -- "$ios_root/../../.." && pwd)
arch=${CURRENT_ARCH:-}
if [ -z "$arch" ] || [ "$arch" = undefined_arch ]; then
  archs=${ARCHS:-}
  arch=${archs%% *}
fi

case "${PLATFORM_NAME:-iphoneos}:$arch" in
  iphoneos:arm64) rust_target=aarch64-apple-ios ;;
  iphonesimulator:arm64) rust_target=aarch64-apple-ios-sim ;;
  iphonesimulator:x86_64) rust_target=x86_64-apple-ios ;;
  *) echo "Unsupported Apple build target: ${PLATFORM_NAME:-unset}:$arch" >&2; exit 1 ;;
esac

rustup target add "$rust_target"
cd "$workspace"
cargo build --locked --release -p http-capture-ios --target "$rust_target"
mkdir -p "$ios_root/build/native"
cp "$workspace/target/$rust_target/release/libhttp_capture_ios.a" \
   "$ios_root/build/native/libhttp_capture_ios.a"
