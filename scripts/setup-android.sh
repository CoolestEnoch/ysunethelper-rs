#!/usr/bin/env bash
set -euo pipefail
# Install only the requested NDK: the retired SDK package "tools" must never
# be requested (setup-android@v3 installs it by default).
ndk_version=27.2.12479018
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-${RUNNER_TEMP:?}/android-sdk}}"
mkdir -p "$sdk"
manager=''
if [[ -x "$sdk/cmdline-tools/latest/bin/sdkmanager" ]]; then
  manager="$sdk/cmdline-tools/latest/bin/sdkmanager"
else
  shopt -s nullglob
  candidates=("$sdk"/cmdline-tools/*/bin/sdkmanager)
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then manager="$candidate"; fi
  done
fi
if [[ -z "$manager" ]]; then
  stage="$(mktemp -d)"
  trap 'rm -rf "$stage"' EXIT
  curl --fail --location --retry 3 \
    https://dl.google.com/android/repository/commandlinetools-linux-12266719_latest.zip \
    -o "$stage/cmdline-tools.zip"
  unzip -q "$stage/cmdline-tools.zip" -d "$stage"
  mkdir -p "$sdk/cmdline-tools"
  mv "$stage/cmdline-tools" "$sdk/cmdline-tools/ysunethelper"
  manager="$sdk/cmdline-tools/ysunethelper/bin/sdkmanager"
fi
export ANDROID_HOME="$sdk" ANDROID_SDK_ROOT="$sdk"
# Process substitution avoids the expected yes/SIGPIPE becoming a pipefail.
"$manager" --sdk_root="$sdk" --licenses < <(yes) >/dev/null
"$manager" --sdk_root="$sdk" "ndk;$ndk_version"
test -x "$sdk/ndk/$ndk_version/toolchains/llvm/prebuilt/linux-x86_64/bin/clang"
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'ANDROID_HOME=%s\nANDROID_SDK_ROOT=%s\nANDROID_NDK_HOME=%s/ndk/%s\n' "$sdk" "$sdk" "$sdk" "$ndk_version" >> "$GITHUB_ENV"
fi
if [[ -n "${GITHUB_PATH:-}" ]]; then
  dirname "$manager" >> "$GITHUB_PATH"
fi
