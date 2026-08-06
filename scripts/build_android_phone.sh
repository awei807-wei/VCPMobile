#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

source "$ROOT_DIR/scripts/android_env.sh"

APK_PATH="$ROOT_DIR/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk"

pnpm tauri android build --apk --target aarch64
python3 "$ROOT_DIR/scripts/verify_android_apk.py" "$APK_PATH" --expected-abi arm64-v8a

APK_SIGNER="$(find "$ANDROID_HOME/build-tools" -name apksigner -type f | sort -V | tail -n 1)"
ZIP_ALIGN="$(find "$ANDROID_HOME/build-tools" -name zipalign -type f | sort -V | tail -n 1)"

if [[ -z "$APK_SIGNER" || -z "$ZIP_ALIGN" ]]; then
  printf '缺少 apksigner 或 zipalign，请安装 Android build-tools。\n' >&2
  exit 1
fi

"$APK_SIGNER" verify -v "$APK_PATH"
"$ZIP_ALIGN" -c -P 16 4 "$APK_PATH"

CERTIFICATE_INFO="$("$APK_SIGNER" verify --print-certs "$APK_PATH")"
if [[ "$CERTIFICATE_INFO" == *"Android Debug"* ]]; then
  printf '警告: 当前 APK 使用 Android Debug 证书，只适合本地真机回归，不能作为正式发布包。\n' >&2
fi

printf '\n真机 APK 已生成并通过校验:\n%s\n' "$APK_PATH"
