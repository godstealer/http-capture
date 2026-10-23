#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
if [[ -z "$sdk" ]]; then
  echo 'ANDROID_SDK_ROOT or ANDROID_HOME must point to the Android SDK' >&2
  exit 1
fi

ndk="${ANDROID_NDK_HOME:-}"
if [[ -z "$ndk" ]]; then
  if [[ ! -d "$sdk/ndk" ]]; then
    echo "Android NDK is missing under $sdk/ndk" >&2
    exit 1
  fi
  ndk="$(find "$sdk/ndk" -mindepth 1 -maxdepth 1 -type d -print | sort -V | tail -n 1)"
fi
if [[ -z "$ndk" || ! -d "$ndk" ]]; then
  echo 'Android NDK is required (set ANDROID_NDK_HOME)' >&2
  exit 1
fi

toolchain="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
if [[ ! -d "$toolchain" ]]; then
  echo "Android NDK Linux toolchain is missing: $toolchain" >&2
  exit 1
fi

configuration="${1:-debug}"
architecture="${2:-both}"
case "$configuration" in debug|release) ;; *) echo "Invalid configuration: $configuration" >&2; exit 1 ;; esac
case "$architecture" in arm64-v8a|x86_64|both) ;; *) echo "Invalid architecture: $architecture" >&2; exit 1 ;; esac

export PATH="$toolchain:$PATH"
rustup target add aarch64-linux-android x86_64-linux-android

build_one() {
  local abi="$1" rust_arch="$2" target="${2}-linux-android"
  local env_arch="${rust_arch}_linux_android"
  local compiler="$toolchain/${rust_arch}-linux-android26-clang"
  if [[ "$rust_arch" == x86_64 ]]; then
    compiler="$toolchain/x86_64-linux-android26-clang"
  fi
  if [[ ! -x "$compiler" ]]; then
    echo "Android compiler is missing: $compiler" >&2
    exit 1
  fi
  export "CC_${env_arch}=$compiler"
  export "AR_${env_arch}=$toolchain/llvm-ar"
  export "BINDGEN_EXTRA_CLANG_ARGS_${env_arch}=--target=$target"
  local linker_name="CARGO_TARGET_${env_arch^^}_LINKER"
  export "$linker_name=$compiler"
  local args=(build --locked -p http-capture-android --target "$target")
  if [[ "$configuration" == release ]]; then args+=(--release); fi
  (cd "$root" && cargo "${args[@]}")
  local destination="$root/apps/mobile/android/app/src/main/jniLibs/$abi"
  mkdir -p "$destination"
  cp "$root/target/$target/$configuration/libhttp_capture_android.so" "$destination/"
  "$toolchain/llvm-strip" --strip-debug "$destination/libhttp_capture_android.so"
}

if [[ "$architecture" == both || "$architecture" == arm64-v8a ]]; then
  build_one arm64-v8a aarch64
fi
if [[ "$architecture" == both || "$architecture" == x86_64 ]]; then
  build_one x86_64 x86_64
fi

task=assembleDebug
if [[ "$configuration" == release ]]; then task=assembleRelease; fi
gradle -p "$root/apps/mobile/android" ":app:$task" --no-daemon
