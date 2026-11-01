#!/usr/bin/env bash
# 배포 폴더의 sources/ 만들기: 앱에 든 LGPL·MPL 부품의 정확한 소스와 빌드·줄이기 방법(공개 소스 저장소에 올리는 묶음).
# 사용: bash scripts/make-sources.sh <출력 폴더> <앱 버전>
#   - ffmpeg·LAME·dav1d·zlib 소스 묶음 그대로 + configure.txt + build-ffmpeg.sh (third_party/ffmpeg/sources, build-ffmpeg.sh가 만듦)
#   - LibreOffice 소스 묶음(본체 + 번역) + 서명(.asc) + trim-libreoffice.ps1 (scripts/fetch-lo-source.sh가 받음)
#   - PDFium 바이너리 출처·판·빌드 설정(pdfium-build.txt)
#   - README.md(한국어 + 짧은 영어), SHA256SUMS.txt
# SOURCES_SKIP_LO=1이면 LibreOffice 소스 묶음을 빼고(개발용 빌드) README에 그렇게 적는다.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?출력 폴더}
APPVER=${2:?앱 버전}
mkdir -p "$OUT"
FF=third_party/ffmpeg/sources
cp -l "$FF"/*.tar.* "$OUT/" 2>/dev/null || cp "$FF"/*.tar.* "$OUT/"
cp "$FF/configure.txt" scripts/build-ffmpeg.sh scripts/trim-libreoffice.ps1 "$OUT/"
LOV=$(grep -a -o 'ooSetupVersionAboutBox"><value>[0-9.]*' third_party/libreoffice/lo-trim/share/registry/main.xcd | sed 's/.*<value>//' | head -1)
LO_NOTE=""
if [ "${SOURCES_SKIP_LO:-}" = 1 ]; then
  LO_NOTE="(이 묶음에는 LibreOffice 소스를 넣지 않았습니다 — 개발용 빌드)"
else
  bash scripts/fetch-lo-source.sh
  for f in "libreoffice-$LOV.tar.xz" "libreoffice-translations-$LOV.tar.xz"; do
    cp -l "third_party/lo-src/$f" "$OUT/" 2>/dev/null || cp "third_party/lo-src/$f" "$OUT/"
    cp "third_party/lo-src/$f.asc" "$OUT/"
  done
fi
P=third_party/pdfium/win-x64
PDFV=$(. "$P/VERSION"; echo "$MAJOR.$MINOR.$BUILD.$PATCH")
PDFSHA=$(grep -o '\[win-x64\]="[0-9a-f]*"' scripts/fetch-pdfium.sh | sed 's/.*="//; s/"//')
{
  echo "PDFium $PDFV (chromium/7881) — BSD-3-Clause (함께 든 서드파티 라이선스는 앱의 THIRD-PARTY-NOTICES.txt)"
  echo "바이너리: https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz"
  echo "SHA-256(pdfium-win-x64.tgz): $PDFSHA"
  echo "소스: https://pdfium.googlesource.com/pdfium/+/refs/heads/chromium/7881"
  echo "빌드 설정(args.gn):"
  sed 's/^/  /' "$P/args.gn"
} > "$OUT/pdfium-build.txt"
FFV=$(ls "$FF" | sed -n 's/^ffmpeg-\(.*\)\.tar\.xz$/\1/p')
cat > "$OUT/README.md" <<MD
# 파일 도구 $APPVER — 함께 든 오픈소스 부품의 소스

이 폴더는 [파일 도구](https://github.com/salarycrew/win-filetool) $APPVER 배포본(win-filetool-$APPVER-portable.exe, win-filetool-$APPVER-setup.exe)에
들어 있는 LGPL·MPL 부품의 **정확한 소스**와 빌드 방법입니다. 파일 도구 자체의 코드(MIT)는 위 저장소에 있습니다.

| 파일 | 무엇 |
|---|---|
| \`ffmpeg-$FFV.tar.xz\` | ffmpeg $FFV 원본 소스(고치지 않음, LGPL-2.1-or-later). 앱의 ffmpeg.exe·ffprobe.exe |
| \`lame-3.100.tar.gz\` | LAME 3.100 원본 소스(LGPL-2.0-or-later, MP3 만들기) — ffmpeg에 정적 링크 |
| \`dav1d-1.5.1.tar.xz\` | dav1d 1.5.1 원본 소스(BSD-2-Clause, AV1 풀기) — ffmpeg에 정적 링크 |
| \`zlib-1.3.1.tar.gz\` | zlib 1.3.1 원본 소스(zlib License) — ffmpeg에 정적 링크 |
| \`configure.txt\` | ffmpeg configure 줄과 함께 링크한 라이브러리 설정 |
| \`build-ffmpeg.sh\` | 위 소스로 Windows x64용 ffmpeg를 Linux에서 교차 빌드하는 스크립트(llvm-mingw) |
| \`libreoffice-$LOV.tar.xz\` | LibreOffice $LOV 본체 소스(MPL-2.0 등). download.documentfoundation.org에서 받아 SHA-256 확인 $LO_NOTE |
| \`libreoffice-translations-$LOV.tar.xz\` | LibreOffice $LOV 번역 소스(앱에 든 한국어 화면 번역의 원본) |
| \`*.asc\` | The Document Foundation의 서명(받은 그대로) |
| \`trim-libreoffice.ps1\` | 공식 LibreOffice $LOV Windows 설치본에서 필요 없는 파일을 빼는 스크립트. 파일 내용은 바꾸지 않음 |
| \`pdfium-build.txt\` | PDFium(BSD-3-Clause) 바이너리 출처·판·SHA-256·빌드 설정 |
| \`SHA256SUMS.txt\` | 이 폴더 파일들의 SHA-256 |

LibreOffice가 빌드 때 쓰는 외부 라이브러리 소스는 본체 소스의 \`download.lst\`에 적혀 있고 https://dev-www.libreoffice.org/src/ 에서 받을 수 있습니다.
ffmpeg는 \`--enable-gpl\`·\`--enable-nonfree\` 없이 빌드했고, LibreOffice는 GPL인 poppler PDF 가져오기 부품을 뺐습니다.

## English (short)

Exact sources of the LGPL/MPL components shipped in File Tool (파일 도구) $APPVER: ffmpeg $FFV + LAME 3.100 + dav1d 1.5.1 + zlib 1.3.1
(unmodified tarballs, configure line in \`configure.txt\`, cross-build script \`build-ffmpeg.sh\`), LibreOffice $LOV (unmodified source tarballs
from download.documentfoundation.org; the app ships the official Windows build with unneeded files removed by \`trim-libreoffice.ps1\`),
and PDFium build info. Checksums are in \`SHA256SUMS.txt\`. The app's own code is MIT: https://github.com/salarycrew/win-filetool
MD
rm -f "$OUT/SHA256SUMS.txt"
{ echo; echo "## SHA-256"; echo; echo '```'; (cd "$OUT" && sha256sum -- $(ls | grep -v -x -e README.md -e SHA256SUMS.txt)); echo '```'; } >> "$OUT/README.md"
(cd "$OUT" && sha256sum -- $(ls | grep -v -x SHA256SUMS.txt) > SHA256SUMS.txt)
du -sh "$OUT"
