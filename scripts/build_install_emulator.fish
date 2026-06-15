#!/usr/bin/env fish

set PROJECT_ROOT (dirname (dirname (status --current-filename)))
set DEVICE emulator-5554
set APP_ID com.vcp.avatar.debug
set TARGET x86_64
set SHOW_LOGS 0

argparse 'd/device=' 't/target=' 'p/package=' 'l/logs' h/help -- $argv
or exit 2

if set -q _flag_help
    echo "Usage: scripts/build_install_emulator.fish [--device emulator-5554] [--target x86_64] [--package com.vcp.avatar.debug] [--logs]"
    exit 0
end

if set -q _flag_device
    set DEVICE $_flag_device
end

if set -q _flag_target
    set TARGET $_flag_target
end

if set -q _flag_package
    set APP_ID $_flag_package
end

if set -q _flag_logs
    set SHOW_LOGS 1
end

function log
    echo "[android-run] $argv"
end

function fail
    echo "[android-run][failed] $argv" >&2
    exit 1
end

source "$PROJECT_ROOT/scripts/android_env.fish"
cd "$PROJECT_ROOT"

command -q adb; or fail "adb 不在 PATH，请先 source scripts/android_env.fish"
command -q pnpm; or fail "pnpm 不在 PATH"

test -d "$ANDROID_HOME"; or fail "ANDROID_HOME 不存在: $ANDROID_HOME"
test -d "$ANDROID_NDK_HOME"; or fail "ANDROID_NDK_HOME 不存在: $ANDROID_NDK_HOME"

if not adb devices | awk '{print $1}' | grep -qx "$DEVICE"
    adb devices -l
    fail "未找到设备: $DEVICE"
end

log "设备: $DEVICE"
log "SDK: $ANDROID_HOME"
log "NDK: $ANDROID_NDK_HOME"
log "构建目标: $TARGET"

log "清理端口 reverse"
adb -s "$DEVICE" reverse tcp:1420 tcp:1420 >/dev/null 2>&1
adb -s "$DEVICE" reverse tcp:1421 tcp:1421 >/dev/null 2>&1

log "构建 Debug APK"
pnpm tauri android build --apk --target "$TARGET" --debug
or fail "构建失败"

set APK (find "$PROJECT_ROOT/src-tauri/gen/android/app/build/outputs/apk" -type f -name '*.apk' | sort | grep '/universal/debug/' | tail -n 1)
if test -z "$APK"
    set APK (find "$PROJECT_ROOT/src-tauri/gen/android/app/build/outputs/apk" -type f -name '*.apk' | sort | tail -n 1)
end

test -n "$APK"; or fail "没有找到 APK 输出"
test -f "$APK"; or fail "APK 不存在: $APK"

log "安装 APK: $APK"
adb -s "$DEVICE" install -r "$APK"
or fail "安装失败"

log "启动 App: $APP_ID"
adb -s "$DEVICE" shell monkey -p "$APP_ID" 1
or fail "启动失败"

log "完成"

if test "$SHOW_LOGS" = 1
    log "打开 logcat，按 Ctrl+C 退出"
    adb -s "$DEVICE" logcat | grep -E "$APP_ID|VCP|Tauri|Rust|panic|ERROR|AndroidRuntime"
end
