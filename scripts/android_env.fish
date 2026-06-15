set -gx ANDROID_HOME (set -q ANDROID_HOME_OVERRIDE; and echo $ANDROID_HOME_OVERRIDE; or echo $HOME/Android/Sdk)
set -gx ANDROID_SDK_ROOT $ANDROID_HOME
set -gx ANDROID_NDK_HOME (set -q ANDROID_NDK_HOME_OVERRIDE; and echo $ANDROID_NDK_HOME_OVERRIDE; or echo $ANDROID_HOME/ndk/27.2.12479018)
set -gx NDK_HOME $ANDROID_NDK_HOME
set -gx VCP_ANDROID_NDK_BIN "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"

if not set -q JAVA_HOME
    set -gx JAVA_HOME /usr/lib/jvm/java-17-openjdk
end

fish_add_path -g $ANDROID_HOME/cmdline-tools/latest/bin
fish_add_path -g $ANDROID_HOME/platform-tools
fish_add_path -g $JAVA_HOME/bin
fish_add_path -g $VCP_ANDROID_NDK_BIN

set -gx CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER "$VCP_ANDROID_NDK_BIN/aarch64-linux-android26-clang"
set -gx CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER "$VCP_ANDROID_NDK_BIN/armv7a-linux-androideabi26-clang"
set -gx CARGO_TARGET_I686_LINUX_ANDROID_LINKER "$VCP_ANDROID_NDK_BIN/i686-linux-android26-clang"
set -gx CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER "$VCP_ANDROID_NDK_BIN/x86_64-linux-android26-clang"

set -gx CC_aarch64_linux_android "$VCP_ANDROID_NDK_BIN/aarch64-linux-android26-clang"
set -gx CC_armv7_linux_androideabi "$VCP_ANDROID_NDK_BIN/armv7a-linux-androideabi26-clang"
set -gx CC_i686_linux_android "$VCP_ANDROID_NDK_BIN/i686-linux-android26-clang"
set -gx CC_x86_64_linux_android "$VCP_ANDROID_NDK_BIN/x86_64-linux-android26-clang"
set -gx AR_aarch64_linux_android "$VCP_ANDROID_NDK_BIN/llvm-ar"
set -gx AR_armv7_linux_androideabi "$VCP_ANDROID_NDK_BIN/llvm-ar"
set -gx AR_i686_linux_android "$VCP_ANDROID_NDK_BIN/llvm-ar"
set -gx AR_x86_64_linux_android "$VCP_ANDROID_NDK_BIN/llvm-ar"

set -l VCP_RUSTFLAGS_16K "-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
if set -q RUSTFLAGS
    if not string match -q "*max-page-size=16384*" -- "$RUSTFLAGS"
        set -gx RUSTFLAGS "$VCP_RUSTFLAGS_16K $RUSTFLAGS"
    end
else
    set -gx RUSTFLAGS "$VCP_RUSTFLAGS_16K"
end

set -gx NO_PROXY 127.0.0.1,localhost,10.0.2.2,10.0.0.188,10.0.0.189,$NO_PROXY
set -gx no_proxy $NO_PROXY
