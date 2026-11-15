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
ndk="$(cd "$ndk" && pwd)"
export ANDROID_NDK_HOME="$ndk"
if [[ ! -f "$ndk/build/cmake/android.toolchain.cmake" ]]; then
  echo "Android CMake toolchain is missing under $ndk" >&2
  exit 1
fi

toolchain="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
if [[ ! -d "$toolchain" ]]; then
  echo "Android NDK Linux toolchain is missing: $toolchain" >&2
  exit 1
fi
readelf="$toolchain/llvm-readelf"
if [[ ! -x "$readelf" ]]; then
  echo "Android ELF inspector is missing: $readelf" >&2
  exit 1
fi

verify_android_library() {
  local library="$1" expected_machine="$2" header
  header="$("$readelf" --file-header "$library")"
  if ! grep -Eq "Machine:[[:space:]]+$expected_machine([[:space:]]|$)" <<<"$header"; then
    echo "Android library has the wrong ELF architecture: $library (expected $expected_machine)" >&2
    return 1
  fi
}

print_btls_cmake_diagnostics() {
  local build_dir="$1" diagnostic
  [[ -d "$build_dir" ]] || return 0
  while IFS= read -r -d '' diagnostic; do
    echo "BoringSSL CMake diagnostic: $diagnostic" >&2
    case "$diagnostic" in
      */CMakeCache.txt)
        grep -E '^(ANDROID_|CMAKE_(TOOLCHAIN_FILE|SYSTEM_|CXX_COMPILER|C_COMPILER|ASM_COMPILER|GENERATOR|SYSROOT|CXX_FLAGS))' "$diagnostic" >&2 || true
        ;;
      *)
        grep -E '^set\(CMAKE_CXX_(COMPILER|PLATFORM_ID|SIMULATE_ID|SIZEOF_DATA_PTR|LIBRARY_ARCHITECTURE)' "$diagnostic" >&2 || true
        ;;
    esac
  done < <(find "$build_dir" -type f \( -path '*/btls-sys*/out/build/CMakeCache.txt' -o -path '*/btls-sys*/out/build/CMakeFiles/*/CMakeCXXCompiler.cmake' \) -print0)
}

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
  local cxx="$toolchain/${rust_arch}-linux-android26-clang++"
  local runtime="$toolchain/../sysroot/usr/lib/$target/libc++_shared.so"
  local expected_machine="AArch64"
  if [[ "$rust_arch" == x86_64 ]]; then expected_machine="Advanced Micro Devices X86-64"; fi
  if [[ "$rust_arch" == x86_64 ]]; then
    compiler="$toolchain/x86_64-linux-android26-clang"
  fi
  if [[ ! -x "$compiler" ]]; then
    echo "Android compiler is missing: $compiler" >&2
    exit 1
  fi
  if [[ ! -x "$cxx" ]]; then
    echo "Android C++ compiler is missing: $cxx" >&2
    exit 1
  fi
  if [[ ! -f "$runtime" ]]; then
    echo "Android C++ runtime is missing: $runtime" >&2
    exit 1
  fi
  local cmake_toolchain="$root/target/android-toolchains/$target.cmake"
  mkdir -p "$(dirname "$cmake_toolchain")"
  cat > "$cmake_toolchain" <<EOF
set(ANDROID_ABI "$abi" CACHE STRING "" FORCE)
set(ANDROID_PLATFORM "android-26" CACHE STRING "" FORCE)
set(ANDROID_STL "c++_shared" CACHE STRING "" FORCE)
include("$ndk/build/cmake/android.toolchain.cmake")
EOF
  export "CMAKE_TOOLCHAIN_FILE_${env_arch}=$cmake_toolchain"
  export "CC_${env_arch}=$compiler"
  export "CXX_${env_arch}=$cxx"
  export "AR_${env_arch}=$toolchain/llvm-ar"
  export "BINDGEN_EXTRA_CLANG_ARGS_${env_arch}=--target=$target"
  local linker_name="CARGO_TARGET_${env_arch^^}_LINKER"
  export "$linker_name=$compiler"
  local args=(build --locked -p http-capture-android --target "$target")
  if [[ "$configuration" == release ]]; then args+=(--release); fi
  if (cd "$root" && cargo "${args[@]}"); then
    :
  else
    local cargo_status=$?
    print_btls_cmake_diagnostics "$root/target/$target/$configuration/build"
    return "$cargo_status"
  fi
  local destination="$root/apps/mobile/android/app/src/main/jniLibs/$abi"
  mkdir -p "$destination"
  cp "$root/target/$target/$configuration/libhttp_capture_android.so" "$destination/"
  cp "$runtime" "$destination/libc++_shared.so"
  "$toolchain/llvm-strip" --strip-debug "$destination/libhttp_capture_android.so"
  verify_android_library "$destination/libhttp_capture_android.so" "$expected_machine"
  verify_android_library "$destination/libc++_shared.so" "$expected_machine"
  local dynamic needed
  dynamic="$("$readelf" --dynamic "$destination/libhttp_capture_android.so")"
  while IFS= read -r needed; do
    case "$needed" in
      libc++*)
        if [[ ! -f "$destination/$needed" ]]; then
          echo "Required Android C++ runtime is not packaged: $needed ($abi)" >&2
          exit 1
        fi
        ;;
    esac
  done < <(sed -n 's/.*(NEEDED).*Shared library: \[\([^]]*\)\].*/\1/p' <<<"$dynamic")
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
