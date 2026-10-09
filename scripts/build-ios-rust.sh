#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
export IPHONEOS_DEPLOYMENT_TARGET=16.0
cargo build -p xcoc-mobile-ffi --release --locked --target aarch64-apple-ios
cargo build -p xcoc-mobile-ffi --release --locked --target aarch64-apple-ios-sim
vendor="$root/clients/ios/Vendor"
mkdir -p "$vendor"
if test -d "$vendor/XcocRust.xcframework"; then
    # This directory contains only generated build output.
    rm -rf "$vendor/XcocRust.xcframework"
fi
xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libxcoc_mobile_ffi.a -headers ffi/include \
    -library target/aarch64-apple-ios-sim/release/libxcoc_mobile_ffi.a -headers ffi/include \
    -output "$vendor/XcocRust.xcframework"
