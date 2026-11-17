#!/bin/sh
# Build an unsigned iOS app bundle. Requires Xcode's iphoneos SDK.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
sdk=$(xcrun --sdk iphoneos --show-sdk-path)
out="$root/target/ios/MithuLite.app"
rm -rf "$out"
mkdir -p "$out"
cp "$root/mobile/ios/Info.plist" "$out/Info.plist"
sh "$root/tools/install-mobile-assets.sh" "$out"
xcrun --sdk iphoneos swiftc \
  -target arm64-apple-ios16.0 \
  -sdk "$sdk" \
  -framework UIKit \
  -framework WebKit \
  -parse-as-library \
  -o "$out/MithuLite" \
  "$root/mobile/ios/MithuLite.swift"
echo "unsigned ios ok: $out"
