#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
: "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to an installed Android NDK (r28 or newer)}"
cargo ndk -t arm64-v8a -o clients/android/app/src/main/jniLibs build -p xcoc-mobile-ffi --release --locked
