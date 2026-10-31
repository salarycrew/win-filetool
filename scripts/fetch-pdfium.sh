#!/usr/bin/env bash
# PDF → 이미지 렌더러(PDFium) 바이너리를 받는다. 저장소에는 넣지 않는다(third_party/는 .gitignore).
# 출처: https://github.com/bblanchon/pdfium-binaries (PDFium: BSD-3-Clause, 함께 든 서드파티 라이선스는 LICENSE 참고)
# 버전은 pdfium-render 0.9.4의 기본 API(pdfium_7881)에 맞춘 chromium/7881로 고정한다.
set -euo pipefail
cd "$(dirname "$0")/.."
TAG="chromium/7881"
BASE="https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG"
declare -A SHA=(
  [win-x64]="73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08"
  [linux-x64]="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"
)
for plat in win-x64 linux-x64; do
  dir="third_party/pdfium/$plat"
  [ -f "$dir/.done" ] && continue
  mkdir -p "$dir"
  tgz="third_party/pdfium/pdfium-$plat.tgz"
  curl -sSfL -o "$tgz" "$BASE/pdfium-$plat.tgz"
  got=$(sha256sum "$tgz" | cut -d' ' -f1)
  if [ "$got" != "${SHA[$plat]}" ]; then
    echo "sha256이 다릅니다 ($plat): $got" >&2; exit 1
  fi
  tar xzf "$tgz" -C "$dir"
  rm -f "$tgz"
  touch "$dir/.done"
done
ls -l third_party/pdfium/win-x64/bin/pdfium.dll third_party/pdfium/linux-x64/lib/libpdfium.so
