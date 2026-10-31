#!/usr/bin/env bash
# 설치 파일(NSIS)을 만들 makensis를 sudo 없이 준비한다(K12, Ubuntu).
# apt-get download로 nsis/nsis-common 패키지를 받아 ~/.local/opt/nsis-3.10에 풀고,
# NSISDIR을 맞춰 주는 makensis 감싸개를 만든다. Tauri bundler는 PATH의 makensis를 쓴다.
set -euo pipefail
DEST="${NSIS_HOME:-$HOME/.local/opt/nsis-3.10}"
if [ ! -x "$DEST/usr/bin/makensis" ]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  (cd "$tmp" && apt-get download nsis nsis-common)
  mkdir -p "$DEST"
  for f in "$tmp"/*.deb; do dpkg-deb -x "$f" "$DEST"; done
fi
mkdir -p "$DEST/wrap"
cat > "$DEST/wrap/makensis" <<WRAP
#!/bin/sh
NSISDIR="$DEST/usr/share/nsis" exec "$DEST/usr/bin/makensis" "\$@"
WRAP
chmod +x "$DEST/wrap/makensis"
"$DEST/wrap/makensis" -VERSION; echo
echo "PATH에 추가: $DEST/wrap"
