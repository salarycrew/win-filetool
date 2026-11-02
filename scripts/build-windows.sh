#!/usr/bin/env bash
# Linux 빌드 PC에서 Windows용 exe·설치 파일을 크로스컴파일한다 (Tauri 문서의 cargo-xwin 방식).
# 필요(모두 사용자 영역, sudo 불필요):
#   - rustup + target x86_64-pc-windows-msvc
#   - cargo-xwin (cargo install --locked cargo-xwin) — MSVC CRT/Windows SDK를 ~/.cache/cargo-xwin에 내려받음
#   - LLVM(clang-cl, lld-link, llvm-lib, llvm-rc): LLVM_HOME (기본 ~/.local/opt/LLVM-23.1.4-Linux-X64)
#   - node/npm (npm ci)
#   - 1c: PDFium(scripts/fetch-pdfium.sh → third_party/pdfium), makensis(scripts/fetch-nsis.sh → ~/.local/opt/nsis-3.10)
#   - 0.3.0: ffmpeg LGPL 빌드(scripts/build-ffmpeg.sh → third_party/ffmpeg, 없으면 여기서 빌드),
#            줄인 LibreOffice(scripts/trim-libreoffice.ps1 결과를 third_party/libreoffice/lo-trim/에 둔다), zstd, python3
#   - 0.3.3: LibreOffice는 앱에 넣지 않고 "문서 도구" 묶음(third_party/doctools/win-filetool-doctools-1.zst)으로 따로 둔다.
#            없으면 lo-trim에서 만들고, SHA-256이 앱에 고정한 값(crates/core/src/doctools.rs)과 다르면 멈춘다.
# 결과(dist-win/):
#   - 파일도구_<버전>_portable.exe   설치 없는 exe 하나(도구 묶음 tools.pack — ffmpeg·PDFium·고지 — 을 exe 뒤에 덧붙임)
#   - win-filetool-doctools-1.zst(+ .sha256)  문서 도구 묶음(릴리스에 따로 올림, 앱이 필요할 때 받음)
#   - 파일도구_<버전>_x64-setup.exe  사용자별 설치 파일(관리자 권한 불필요, tools.pack·filetool-svc.exe 포함)
#   - THIRD-PARTY-NOTICES.txt, sources/(ffmpeg·LAME·dav1d·zlib 소스 묶음 그대로 + configure 줄 + 빌드·줄이기 스크립트)
#   - cli/filetool-cli.exe            점검·자동화용(휴대용 exe와 같은 폴더에 두면 그 안의 도구를 씀)
set -euo pipefail
cd "$(dirname "$0")/.."

LLVM_HOME="${LLVM_HOME:-$HOME/.local/opt/LLVM-23.1.4-Linux-X64}"
NSIS_HOME="${NSIS_HOME:-$HOME/.local/opt/nsis-3.10}"
export PATH="$HOME/.cargo/bin:$LLVM_HOME/bin:$NSIS_HOME/wrap:$PATH"
TARGET=x86_64-pc-windows-msvc
REL="target/$TARGET/release"

for tool in cargo cargo-xwin clang-cl lld-link llvm-lib llvm-rc makensis; do
  command -v "$tool" >/dev/null || { echo "필요한 도구가 없습니다: $tool (makensis는 scripts/fetch-nsis.sh)" >&2; exit 1; }
done

[ -d node_modules ] || npm ci
bash scripts/fetch-pdfium.sh >/dev/null
[ -f third_party/ffmpeg/win-x64/bin/ffmpeg.exe ] || bash scripts/build-ffmpeg.sh
command -v zstd >/dev/null || { echo "필요한 도구가 없습니다: zstd" >&2; exit 1; }
# 0.3.3 문서 도구 묶음: 고정 SHA-256과 같은지(앱이 받은 파일을 이 값으로 확인한다)
DT_ASSET=$(grep -o 'pub const ASSET: &str = "[^"]*"' crates/core/src/doctools.rs | sed 's/.*"\(.*\)"/\1/')
DT_SHA=$(grep -o 'pub const SHA256: &str = "[0-9a-f]*"' crates/core/src/doctools.rs | sed 's/.*"\(.*\)"/\1/')
DT="third_party/doctools/$DT_ASSET"
if [ ! -f "$DT" ]; then
  [ -f third_party/libreoffice/lo-trim/program/soffice.exe ] || {
    echo "문서 도구 묶음($DT)도, 줄인 LibreOffice(third_party/libreoffice/lo-trim)도 없습니다" >&2; exit 1; }
  python3 scripts/make-tools-pack.py --doctools "$DT"
fi
got=$(sha256sum "$DT" | cut -d' ' -f1)
[ "$got" = "$DT_SHA" ] || { echo "문서 도구 묶음 SHA-256이 앱에 고정한 값과 다릅니다: $got (고정 $DT_SHA)" >&2; exit 1; }
echo "문서 도구 묶음: $DT_ASSET $(stat -c %s "$DT") B, SHA-256 고정값과 같음"
# 0.3.3(보안 강화): 0.3.x 폴더 옮겨 쓰기 확인에 쓰는 "폴더 내용 해시"가 이 묶음과 맞는지
DT_TREE=$(grep -o 'pub const LO_TREE_SHA256: &str = "[0-9a-f]*"' crates/core/src/doctools.rs | sed 's/.*"\(.*\)"/\1/')
got_tree=$(python3 scripts/doctools-tree-hash.py "$DT" | cut -d' ' -f1)
[ "$got_tree" = "$DT_TREE" ] || { echo "문서 도구 폴더 내용 해시가 앱의 값과 다릅니다: $got_tree (앱 $DT_TREE)" >&2; exit 1; }
echo "문서 도구 폴더 내용 해시: 앱의 값과 같음"

# 0.3.0: 오픈소스 고지 + 도구 묶음(설치본 리소스와 휴대용 exe 덧붙이기에 같이 씀)
mkdir -p target/tools
python3 scripts/make-notices.py target/tools/THIRD-PARTY-NOTICES.txt ${SOURCE_URL:+--source-url "$SOURCE_URL"}
if [ "${TOOLS_PACK_REUSE:-}" != 1 ] || [ ! -f target/tools/tools.pack ]; then
  python3 scripts/make-tools-pack.py target/tools/tools.pack target/tools/THIRD-PARTY-NOTICES.txt
fi
# 0.3.3: 앱 묶음에 LibreOffice(lo)가 남아 있으면(예전 tools.pack 다시 쓰기) 멈춘다
python3 - target/tools/tools.pack <<'PY'
import json, sys
f = open(sys.argv[1], "rb"); f.seek(0, 2); n = f.tell(); f.seek(n - 24); t = f.read(24)
il = int.from_bytes(t[:8], "little"); f.seek(n - 24 - il); names = [e["name"] for e in json.loads(f.read(il))["tools"]]
print("앱 묶음 도구:", ", ".join(names))
sys.exit(1 if "lo" in names else 0)
PY

# exe 안에 빌드한 PC의 경로(/home/…, ~/.cargo/registry …)가 남지 않게(패닉 위치 문자열)
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME/.cargo/registry/src=crates --remap-path-prefix=$HOME/.cargo/git/checkouts=git --remap-path-prefix=$HOME/.rustup=rustup --remap-path-prefix=$PWD=win-filetool"

# tauri-cli는 tray-icon 기능이 켜져 있으면 호스트(Linux)의 appindicator 라이브러리 위치를 pkg-config로 찾다가
# 없으면 멈춘다(Linux 묶음용 정보라 Windows 결과물과는 무관). 이 빌드 PC에 pkg-config가 없을 수 있으므로, 빈 라이브러리 경로만
# 알려 주는 임시 pkg-config로 넘어가게 한다.
STUB=$(mktemp -d)
trap 'rm -rf "$STUB"' EXIT
touch "$STUB/libayatana-appindicator3.so.1"
cat > "$STUB/pkg-config" <<PC
#!/bin/sh
case "\$*" in *ayatana-appindicator3*) echo "-L$STUB" ;; *) exit 1 ;; esac
PC
chmod +x "$STUB/pkg-config"
export PATH="$STUB:$PATH" TAURI_LINUX_AYATANA_APPINDICATOR=true

# 점검·자동화용 동반 CLI + 2c 빠른 검색 서비스(설치 파일에 들어가므로 앱보다 먼저)
cargo xwin build --release --target "$TARGET" -p filetool-cli -p filetool-svc
# 0.3.3(보안 강화): 서비스 exe에 빌드 확인값(자기 SHA-256)을 넣는다 — 설치 때 복사할 파일이 실행 중인 것과 같은지 본다
python3 scripts/patch-svc-hash.py "$REL/filetool-svc.exe"
# GUI 앱 + NSIS 설치 파일(프런트엔드 npm run build까지 tauri CLI가 처리)
npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis

VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
rm -rf dist-win && mkdir -p dist-win/cli
setup=$(ls "$REL"/bundle/nsis/*-setup.exe | head -1)
# 배포 파일 이름은 ASCII(브라우저·메신저·명령줄에서 깨지지 않게). 창 제목·시작 메뉴 이름은 "파일 도구" 그대로.
SETUP="dist-win/win-filetool-${VERSION}-setup.exe"
PORT="dist-win/win-filetool-${VERSION}-portable.exe"
cp "$setup" "$SETUP"
cat "$REL/win-filetool.exe" target/tools/tools.pack > "$PORT"
cp "$REL/filetool-cli.exe" dist-win/cli/
cp target/tools/THIRD-PARTY-NOTICES.txt dist-win/
cp LICENSE dist-win/LICENSE.txt
cp "$DT" "dist-win/$DT_ASSET"
(cd dist-win && sha256sum "$DT_ASSET" > "$DT_ASSET.sha256")
# LGPL·MPL 부품의 정확한 소스(공개 소스 저장소에 올리는 묶음). SOURCES_SKIP_LO=1이면 LibreOffice 소스(약 530 MB)는 뺌
bash scripts/make-sources.sh dist-win/sources "$VERSION"

# 1) 빌드 자리표시({{SOURCE_URL}} 같은)가 남으면 실패(0.3.0 QA-03)
ph=0
for f in "$REL/win-filetool.exe" dist-win/THIRD-PARTY-NOTICES.txt dist/index.html dist/assets/*; do
  if grep -a -q -E '\{\{[A-Z_]+\}\}' "$f"; then echo "오류: $f 에 빌드 자리표시가 남았습니다: $(grep -a -o -E '\{\{[A-Z_]+\}\}' "$f" | sort -u | tr '\n' ' ')" >&2; ph=1; fi
done
[ "$ph" = 0 ] || exit 1
echo "자리표시 점검: 없음"

# 2) 개인 정보·빌드 PC 정보가 들어가지 않았는지(배포 파일·고지·소스 묶음의 스크립트)
# 빌드한 사람의 이름·메일(git 설정)과 빌드 PC 경로·이름을 찾는다(스크립트에 개인 정보를 적어 두지 않으려고 실행할 때 읽음)
me=$(git config user.email 2>/dev/null || true); me_re=${me:+|$(printf '%s' "$me" | sed 's/[.[\*^$()+?{}|]/\\&/g')}
host=$(hostname 2>/dev/null || true)
u=$(id -un); user_re="|[\\\\/]Users[\\\\/]$u\\b|PC\\\\$u\\b"
# LEAK_EXTRA: 더 찾을 낱말(예: 시험 PC 사용자 이름) — 정규식, 스크립트에는 적지 않고 실행할 때 준다
extra_re=${LEAK_EXTRA:+|$LEAK_EXTRA}
leak=0
for f in "$PORT" "$SETUP" "$REL/win-filetool.exe" "$REL/filetool-cli.exe" "$REL/filetool-svc.exe" dist-win/THIRD-PARTY-NOTICES.txt dist-win/sources/*.sh dist-win/sources/*.ps1 dist-win/sources/*.txt dist-win/sources/README.md; do
  n=$(grep -a -c -i -E "/home/[a-z]|C:\\\\Users\\\\[a-z]|\\.cargo/registry$me_re$user_re$extra_re" "$f" || true)
  # 짧은 이름(빌드 PC 이름)은 압축된 묶음(휴대용 exe 뒤 도구 묶음·설치 파일)에서 우연히 맞을 수 있어 압축 안 된 파일에서만
  if [ -n "$host" ]; then case "$f" in "$PORT"|"$SETUP") ;; *) n=$((n + $(grep -a -c -w -i -F "$host" "$f" || true))) ;; esac; fi
  if [ "$n" != 0 ]; then echo "경고: $f 에 로컬 경로·개인 정보로 보이는 문자열 $n줄" >&2; leak=1; fi
done
[ "$leak" = 0 ] || { echo "개인 정보 점검 실패" >&2; exit 1; }
echo "개인 경로 점검: 없음"
# 3) 0.3.3(보안 강화): 앱·CLI·서비스는 정적 가져오기 DLL을 System32에서만(DependentLoadFlags 0x800)
if command -v llvm-readobj >/dev/null; then
  for f in "$REL/win-filetool.exe" "$REL/filetool-cli.exe" "$REL/filetool-svc.exe"; do
    dl=$(llvm-readobj --coff-load-config "$f" | sed -n 's/.*DependentLoadFlags: //p')
    [ "$dl" = "0x800" ] || { echo "오류: $f DependentLoadFlags=$dl (0x800이어야 함)" >&2; exit 1; }
  done
  echo "DLL 검색 점검: DependentLoadFlags 0x800 (앱·CLI·서비스)"
fi
(cd dist-win && sha256sum "$(basename "$PORT")" "$(basename "$SETUP")" "$DT_ASSET" > SHA256SUMS.txt && cat SHA256SUMS.txt)
ls -l dist-win dist-win/cli
sha256sum dist-win/cli/*.exe target/tools/tools.pack
