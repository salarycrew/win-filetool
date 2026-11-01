# 만들기 (개발자용)

파일 도구는 **Tauri 2**(Rust 코어 + Vite/TypeScript 화면) 앱입니다. Windows용 exe는 Linux에서 교차 컴파일하고, Windows PC에서는 실행·점검만 합니다.
설계와 단계별 기록은 [design.md](design.md).

```
crates/core   filetool-core — 변환·PDF 편집·검색 코어(Tauri와 무관, Linux에서 cargo test)
crates/cli    filetool-cli  — 같은 코어의 명령줄 도구(selftest, 변환, 검색…)
crates/svc    filetool-svc  — 선택 설치 빠른 검색 서비스(Windows 서비스)
src-tauri     win-filetool  — 앱(Tauri 명령, 트레이, 창)
src/          화면(TypeScript, UI 프레임워크 없음)
scripts/      빌드·도구 준비 스크립트
```

## Linux 빌드 PC 준비 (한 번, sudo 없이 사용자 영역)

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
~/.cargo/bin/rustup target add x86_64-pc-windows-msvc
~/.cargo/bin/cargo install --locked cargo-xwin     # MSVC CRT·Windows SDK는 첫 빌드 때 ~/.cache/cargo-xwin에 받음
# LLVM(clang-cl, lld-link, llvm-lib, llvm-rc): 공식 릴리스를 ~/.local/opt에 풀기 (LLVM_HOME으로 바꿀 수 있음)
curl -L https://github.com/llvm/llvm-project/releases/download/llvmorg-23.1.4/LLVM-23.1.4-Linux-X64.tar.xz | tar xJ -C ~/.local/opt
bash scripts/fetch-nsis.sh       # 설치 파일용 makensis(~/.local/opt/nsis-3.10)
```

node/npm, python3, zstd가 필요합니다.

## 앱에 넣는 도구

| 도구 | 준비 | 위치(저장소에 넣지 않음) |
|---|---|---|
| PDFium | `bash scripts/fetch-pdfium.sh`(SHA-256 고정, build-windows.sh가 부름) | `third_party/pdfium/` |
| ffmpeg(LGPL 최소 빌드) | `bash scripts/build-ffmpeg.sh`(llvm-mingw 교차 빌드, 없으면 build-windows.sh가 부름) | `third_party/ffmpeg/` |
| LibreOffice(줄인 판) — 0.3.3부터 앱에 넣지 않음 | Windows PC에서 공식 MSI를 관리자 설치로 풀고(`msiexec /a LibreOffice_<판>_Win_x86-64.msi /qn TARGETDIR=<폴더>`) `scripts/trim-libreoffice.ps1 -Source <폴더> -Dest <출력>` → 결과 폴더를 복사 | `third_party/libreoffice/lo-trim/` |
| 문서 도구 묶음(0.3.3) | `python3 scripts/make-tools-pack.py --doctools third_party/doctools/win-filetool-doctools-1.zst`(lo-trim → tar → zstd, 결정적). SHA-256·크기는 `crates/core/src/doctools.rs`에 고정 — 다르면 빌드가 멈춤. 묶음을 바꾸면 `PACK_VERSION`·`ASSET`·`URL`·`SHA256`·`SIZE`를 같이 올린다 | `third_party/doctools/` |
| LibreOffice 소스 묶음(배포용) | `bash scripts/fetch-lo-source.sh`(판은 lo-trim에서 읽음, SHA-256 확인) | `third_party/lo-src/` |

## 빌드

```sh
npm ci
TMPDIR=$PWD/target/test-temp cargo test -p filetool-core -p filetool-cli -p filetool-svc
npm test                         # 화면 코드 단위 시험(node:test)
npm run build                    # 화면만(tsc + vite)
bash scripts/build-windows.sh    # Windows exe·설치 파일·고지·소스 묶음
```

`build-windows.sh` 결과(`dist-win/`):

| 파일 | 내용 |
|---|---|
| `win-filetool-<판>-portable.exe` | 설치 없는 exe 하나(도구 묶음 — ffmpeg·PDFium·고지 — 를 exe 뒤에 덧붙임, 처음 쓸 때 `%LOCALAPPDATA%`에 풂) |
| `win-filetool-doctools-1.zst` (+ `.sha256`) | 문서 도구(줄인 LibreOffice). 릴리스 `v0.3.3`에 따로 올린다 — 앱이 `https://github.com/salarycrew/win-filetool/releases/download/v0.3.3/win-filetool-doctools-1.zst`에서 받는다 |
| `win-filetool-<판>-setup.exe` | 사용자별 설치 파일(NSIS, 한국어, 관리자 권한 불필요, 빠른 검색 서비스 포함) |
| `SHA256SUMS.txt` | 위 세 파일의 SHA-256 |
| `THIRD-PARTY-NOTICES.txt`, `LICENSE.txt` | 오픈소스 고지(앱 안 [앱 정보]에도 있음), 이 앱의 MIT 라이선스 |
| `sources/` | LGPL·MPL 부품의 정확한 소스·빌드 설정·SHA-256(공개 소스 저장소용, 약 520 MB) |
| `cli/filetool-cli.exe` | 점검·자동화용 명령줄 도구(배포하지 않음) |

환경 변수: `SOURCE_URL`(고지의 소스 주소, 기본 `https://github.com/salarycrew/win-filetool-sources`), `TOOLS_PACK_REUSE=1`(도구 묶음 다시 쓰기),
`SOURCES_SKIP_LO=1`(개발용: LibreOffice 소스 묶음 빼기), `LEAK_EXTRA`(개인 정보 점검에 더할 정규식).
빌드는 고지·화면·exe에 `{{…}}` 자리표시가 남거나, 빌드 PC 경로(`/home/…`, `C:\Users\…`)·사용자 이름·git 메일·호스트 이름이 들어가면 실패합니다.

## Windows PC에서 점검

```sh
scp dist-win/win-filetool-<판>-portable.exe dist-win/cli/filetool-cli.exe <windows-pc>:C:/work/
ssh <windows-pc> "C:\work\filetool-cli.exe selftest C:\work\selftest"     # 휴대용 exe와 같은 폴더면 그 안의 도구를 씀
```

- 화면(GUI)은 ssh 세션(세션 0)에서는 보이지 않습니다. 로그인한 사용자 세션에서 띄우려면 `schtasks /create … /it`로 예약 작업을 만들어 실행합니다.
- 설정·색인을 시험 폴더로 돌리기: `WIN_FILETOOL_CONFIG_DIR`, `WIN_FILETOOL_INDEX_DIR`.
- 0.3.3 시험용 바꾸기는 **CLI와 시험용 앱에서만** 읽는다(배포 앱은 무시): 한글·Office가 없는 것처럼 `WIN_FILETOOL_HIDE_COM=1`
  (한글만 `WIN_FILETOOL_HIDE_HANCOM=1`, Office만 `WIN_FILETOOL_HIDE_OFFICE=1`), 문서 도구 받을 주소 `WIN_FILETOOL_DOCTOOLS_URL=http://127.0.0.1:<포트>/…`.
  시험용 앱: `npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle --features test-hooks`(배포 파일에 쓰지 않음).
  해시는 바꿀 수 없으므로 가짜 서버는 진짜 묶음(맞음) 또는 바꾼 묶음(거절 시험)을 준다.
- 검색 벤치마크(합성 100만 항목): `cargo test --release -p filetool-core --test search_bench -- --ignored --nocapture`

## 배포 이름

창 제목·시작 메뉴는 "파일 도구", 배포 파일 이름은 ASCII(`win-filetool-…`)입니다. 판 번호는 `Cargo.toml`(workspace), `package.json`,
`src-tauri/tauri.conf.json` 세 곳을 같이 바꿉니다.
