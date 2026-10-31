#!/usr/bin/env bash
# K12(Linux)에서 Windows용 exe·설치 파일을 크로스컴파일한다 (Tauri 문서의 cargo-xwin 방식).
# 필요(모두 사용자 영역, sudo 불필요):
#   - rustup + target x86_64-pc-windows-msvc
#   - cargo-xwin (cargo install --locked cargo-xwin) — MSVC CRT/Windows SDK를 ~/.cache/cargo-xwin에 내려받음
#   - LLVM(clang-cl, lld-link, llvm-lib, llvm-rc): LLVM_HOME (기본 ~/.local/opt/LLVM-23.1.4-Linux-X64)
#   - node/npm (npm ci)
#   - 1c: PDFium(scripts/fetch-pdfium.sh → third_party/pdfium), makensis(scripts/fetch-nsis.sh → ~/.local/opt/nsis-3.10)
# 결과(dist-win/):
#   - 파일도구_<버전>_x64-setup.exe  사용자별 설치 파일(관리자 권한 불필요, pdfium.dll 포함)
#   - win-filetool-<버전>-portable/ (+ .zip)  win-filetool.exe, filetool-cli.exe, pdfium.dll, licenses/
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

# tauri-cli는 tray-icon 기능이 켜져 있으면 호스트(Linux)의 appindicator 라이브러리 위치를 pkg-config로 찾다가
# 없으면 멈춘다(Linux 묶음용 정보라 Windows 결과물과는 무관). K12에는 pkg-config도 없으므로, 빈 라이브러리 경로만
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

# GUI 앱 + NSIS 설치 파일(프런트엔드 npm run build까지 tauri CLI가 처리)
npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis
# 점검·자동화용 동반 CLI
cargo xwin build --release --target "$TARGET" -p filetool-cli

VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
rm -rf dist-win && mkdir -p dist-win
setup=$(ls "$REL"/bundle/nsis/*-setup.exe | head -1)
cp "$setup" "dist-win/파일도구_${VERSION}_x64-setup.exe"
P="dist-win/win-filetool-${VERSION}-portable"
mkdir -p "$P/licenses/pdfium"
cp "$REL/win-filetool.exe" "$REL/filetool-cli.exe" third_party/pdfium/win-x64/bin/pdfium.dll "$P/"
cp third_party/pdfium/win-x64/licenses/* "$P/licenses/pdfium/"
(cd dist-win && rm -f "win-filetool-${VERSION}-portable.zip" && python3 -c "import shutil,sys; shutil.make_archive(sys.argv[1], 'zip', '.', sys.argv[1])" "win-filetool-${VERSION}-portable")
ls -l dist-win "$P"
sha256sum dist-win/*.exe dist-win/*.zip "$P"/*.exe "$P"/pdfium.dll
