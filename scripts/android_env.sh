#!/usr/bin/env bash

export ANDROID_HOME="${ANDROID_HOME_OVERRIDE:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME_OVERRIDE:-$ANDROID_HOME/ndk/27.2.12479018}"
export NDK_HOME="$ANDROID_NDK_HOME"
export VCP_ANDROID_NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-17-openjdk}"
export PATH="$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$JAVA_HOME/bin:$VCP_ANDROID_NDK_BIN:$PATH"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$VCP_ANDROID_NDK_BIN/aarch64-linux-android26-clang"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$VCP_ANDROID_NDK_BIN/armv7a-linux-androideabi26-clang"
export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$VCP_ANDROID_NDK_BIN/i686-linux-android26-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$VCP_ANDROID_NDK_BIN/x86_64-linux-android26-clang"

export CC_aarch64_linux_android="$VCP_ANDROID_NDK_BIN/aarch64-linux-android26-clang"
export CC_armv7_linux_androideabi="$VCP_ANDROID_NDK_BIN/armv7a-linux-androideabi26-clang"
export CC_i686_linux_android="$VCP_ANDROID_NDK_BIN/i686-linux-android26-clang"
export CC_x86_64_linux_android="$VCP_ANDROID_NDK_BIN/x86_64-linux-android26-clang"
export AR_aarch64_linux_android="$VCP_ANDROID_NDK_BIN/llvm-ar"
export AR_armv7_linux_androideabi="$VCP_ANDROID_NDK_BIN/llvm-ar"
export AR_i686_linux_android="$VCP_ANDROID_NDK_BIN/llvm-ar"
export AR_x86_64_linux_android="$VCP_ANDROID_NDK_BIN/llvm-ar"

VCP_RUSTFLAGS_16K="-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
if [[ "${RUSTFLAGS:-}" != *"max-page-size=16384"* ]]; then
  export RUSTFLAGS="$VCP_RUSTFLAGS_16K ${RUSTFLAGS:-}"
fi

export NO_PROXY="127.0.0.1,localhost,10.0.2.2,10.0.0.188,10.0.0.189,${NO_PROXY:-}"
export no_proxy="$NO_PROXY"
