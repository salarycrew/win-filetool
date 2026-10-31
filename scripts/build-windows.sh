#!/usr/bin/env bash
# K12(Linux)에서 Windows용 exe를 크로스컴파일한다 (Tauri 문서의 cargo-xwin 방식).
# 필요(모두 사용자 영역, sudo 불필요):
#   - rustup + target x86_64-pc-windows-msvc
#   - cargo-xwin (cargo install --locked cargo-xwin) — MSVC CRT/Windows SDK를 ~/.cache/cargo-xwin에 내려받음
#   - LLVM(clang-cl, lld-link, llvm-lib, llvm-rc): LLVM_HOME (기본 ~/.local/opt/LLVM-23.1.4-Linux-X64)
#   - node/npm (npm ci)
# 결과: target/x86_64-pc-windows-msvc/release/{win-filetool.exe, filetool-cli.exe}
set -euo pipefail
cd "$(dirname "$0")/.."

LLVM_HOME="${LLVM_HOME:-$HOME/.local/opt/LLVM-23.1.4-Linux-X64}"
export PATH="$HOME/.cargo/bin:$LLVM_HOME/bin:$PATH"
TARGET=x86_64-pc-windows-msvc

for tool in cargo cargo-xwin clang-cl lld-link llvm-lib llvm-rc; do
  command -v "$tool" >/dev/null || { echo "필요한 도구가 없습니다: $tool" >&2; exit 1; }
done

[ -d node_modules ] || npm ci

# GUI 앱: 프런트엔드(npm run build)까지 tauri CLI가 처리. 1a는 휴대용 exe만(설치 파일 없음).
npx tauri build --runner cargo-xwin --target "$TARGET" --no-bundle
# 점검·자동화용 동반 CLI
cargo xwin build --release --target "$TARGET" -p filetool-cli

ls -l "target/$TARGET/release/win-filetool.exe" "target/$TARGET/release/filetool-cli.exe"
