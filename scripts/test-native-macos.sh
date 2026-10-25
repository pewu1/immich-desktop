#!/bin/sh
set -eu
if [ "$(uname -s)" != "Darwin" ]; then
  echo "This check requires macOS and its AppKit pasteboard service." >&2
  exit 1
fi
cd "$(dirname "$0")/.."
native_test_binary="$(mktemp "${TMPDIR:-/tmp}/immich-native.XXXXXX")"
trap 'rm -f "$native_test_binary"' EXIT
clang -fobjc-arc -fblocks -mmacosx-version-min=11.0 -framework AppKit -framework UniformTypeIdentifiers -framework CoreGraphics tests/native-macos.m -o "$native_test_binary"
"$native_test_binary" "$PWD/fixtures/drag-test.png" "$@"
clang -fobjc-arc -fblocks -mmacosx-version-min=11.0 -framework AppKit -framework WebKit tests/native-session-macos.m src-tauri/native/session_macos.m -o "$native_test_binary"
"$native_test_binary"
