#!/usr/bin/env bash
# 앱에 든 LibreOffice와 같은 판의 소스 묶음을 받는다(MPL 소스 제공용, 배포 폴더 sources/에 넣음).
# 판은 줄인 LibreOffice(third_party/libreoffice/lo-trim)의 share/registry/main.xcd에서 읽는다.
# 받는 곳: https://download.documentfoundation.org/libreoffice/src/<x.y.z>/ — 같은 곳의 .sha256으로 확인하고 .asc(서명)도 함께 둔다.
# 결과: third_party/lo-src/ (저장소에 넣지 않음, 약 530 MB)
set -euo pipefail
cd "$(dirname "$0")/.."
XCD=third_party/libreoffice/lo-trim/share/registry/main.xcd
FULL=$(grep -a -o 'ooSetupVersionAboutBox"><value>[0-9.]*' "$XCD" | sed 's/.*<value>//' | head -1)
[ -n "$FULL" ] || { echo "LibreOffice 판을 읽지 못했습니다: $XCD" >&2; exit 1; }
SHORT=${FULL%.*}
BASE="https://download.documentfoundation.org/libreoffice/src/$SHORT"
D=third_party/lo-src
mkdir -p "$D"
for f in "libreoffice-$FULL.tar.xz" "libreoffice-translations-$FULL.tar.xz"; do
  for ext in sha256 asc; do [ -s "$D/$f.$ext" ] || curl -sSfL -o "$D/$f.$ext" "$BASE/$f.$ext"; done
  if [ ! -s "$D/$f" ] || ! (cd "$D" && sha256sum -c --status "$f.sha256"); then
    curl -sSfL -C - -o "$D/$f" "$BASE/$f" || curl -sSfL -o "$D/$f" "$BASE/$f"
  fi
  (cd "$D" && sha256sum -c "$f.sha256")
done
echo "$FULL" > "$D/VERSION"
