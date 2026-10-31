#!/usr/bin/env bash
# 아이콘 원본 하나(assets/app-icon.svg)에서 앱·설치 파일·트레이 아이콘을 만든다(tauri icon).
# Windows에 필요한 것(icon.ico: 16·24·32·48·64·256)과 Tauri 설정이 가리키는 PNG만 src-tauri/icons에 둔다.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
npx tauri icon assets/app-icon.svg -o "$tmp" >/dev/null 2>&1
for f in 32x32.png 64x64.png 128x128.png 128x128@2x.png icon.png icon.ico icon.icns; do
  cp "$tmp/$f" src-tauri/icons/$f
done
ls -l src-tauri/icons
