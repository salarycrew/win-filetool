# win-filetool 설계

상태: 1a 구현(브랜치 `salarycrew/stage1a`). **스택 확정: Tauri 2** (Rust 코어 + 웹 UI).

## 단계 나누기

| 단계 | 내용 | 상태 |
|---|---|---|
| **1a** | Tauri 2 뼈대(한국어 UI, 끌어다 놓기, 저장 위치), 맥↔윈도 한글 파일명 NFD→NFC(폴더·zip), PDF 페이지 작업(합치기·나누기·회전·삭제·순서 변경) | 이 브랜치 |
| 1b | 변환: 이미지·HEIC·영상(ffmpeg), 문서·Pages/Numbers/Keynote(LibreOffice), 감시 폴더 자동 변환(`notify`) | 다음 |
| 1c | PDF 압축, 이미지↔PDF, 설치 파일(NSIS) 다듬기 | 다음 |
| 2 | Everything식 전체 파일 즉시 검색 (NTFS MFT/USN, `windows` crate `FSCTL_ENUM_USN_DATA`) | 1단계 출시 뒤 |

범위 밖: PDF·문서 본문 텍스트 수정(사용자 확정).

## 구조

```
crates/core   filetool-core  — Tauri 비의존 코어. Linux에서 cargo test로 검증
  nfc.rs      폴더 NFD→NFC: plan_folder(미리보기) → apply_plan(적용). 자식 먼저, 충돌은 건너뛰고 보고
  zipnfc.rs   맥 zip: 이름 바이트 직접 해석(UTF-8, 아니면 CP949) → NFC. __MACOSX/·._*·.DS_Store 제외,
              '..'/절대 경로 차단, 기존 파일 덮어쓰지 않음. rezip_nfc: NFC 이름 새 zip(데이터는 원본 압축 그대로 복사)
  pdf.rs      lopdf. 페이지 트리를 평평하게(상속 Resources/MediaBox/CropBox/Rotate를 페이지로 복사) 만든 뒤
              Kids를 바꿔 나누기·삭제·순서 변경, merge는 객체 번호를 다시 매겨 합침
  pages.rs    "1-3,5", "5-", "-2", "all", 역순 "5-1"
  paths.rs    출력 이름: 원본 옆(또는 지정 폴더)에 이름_접미사.pdf, 있으면 "이름 (1)" — 항상 create_new
  samples.rs  점검용 샘플(NFD 폴더, 맥식 zip, N쪽 PDF) 생성
crates/cli    filetool-cli.exe — 같은 코어를 명령줄로. `selftest <빈 폴더>`로 실제 디스크에서 전 작업 점검
src-tauri     win-filetool.exe — Tauri 2 앱. 명령마다 spawn_blocking으로 돌려 창이 멈추지 않음.
              오류는 코어의 한국어 메시지를 그대로 화면 결과 칸에 표시
src/, index.html  Vite + TypeScript(프레임워크 없음). 창 제목 "파일 도구"
```

원칙: 원본은 바꾸지 않는다(폴더 이름 바꾸기는 예외이며 미리보기 후 적용). 출력은 새 파일로만 쓰고,
같은 이름이 있으면 `(1)`을 붙인다. NFC 이름 바꾸기에서 같은 NFC 이름이 이미 있으면 건너뛰고 보고한다.

## 의존성

- `unicode-normalization`, `walkdir`, `zip`(deflate만), `encoding_rs`(CP949 이름), `lopdf`(기본 기능 끔), `thiserror`, `serde`
- 앱: `tauri` 2, `tauri-plugin-dialog`(파일·폴더 고르기)
- 1b 예정: `notify`, ffmpeg/LibreOffice/libheif 동봉 호출. 1c 예정: PDF 압축(qpdf 또는 lopdf+이미지 재압축)

## 빌드 경로: K12 크로스 컴파일 → desktop 실행

desktop(Windows)에는 Rust·MSVC Build Tools가 없고 원격 세션은 관리자 권한이 아니다.
그래서 **K12(Linux)에서 `cargo-xwin`으로 `x86_64-pc-windows-msvc` exe를 만들고**, desktop에는 복사만 한다.

K12 준비(한 번, 모두 사용자 영역, sudo 없음):

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
~/.cargo/bin/rustup target add x86_64-pc-windows-msvc
~/.cargo/bin/cargo install --locked cargo-xwin      # MSVC CRT·Windows SDK는 첫 빌드 때 ~/.cache/cargo-xwin에 받음
# LLVM(clang-cl, lld-link, llvm-lib, llvm-rc): 공식 릴리스 tarball을 ~/.local/opt에 풀기 (약 12 GB)
curl -L https://github.com/llvm/llvm-project/releases/download/llvmorg-23.1.4/LLVM-23.1.4-Linux-X64.tar.xz | tar xJ -C ~/.local/opt
```

빌드:

```sh
npm ci
bash scripts/build-windows.sh        # = npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle
                                     #   + cargo xwin build --release -p filetool-cli
# → target/x86_64-pc-windows-msvc/release/win-filetool.exe, filetool-cli.exe
cargo test -p filetool-core          # 코어 테스트(Linux)
```

설치 파일(NSIS)은 1a에서 만들지 않는다(`makensis` 없음, `--no-bundle`). 휴대용 exe 하나로 실행된다(WebView2 런타임 필요 — desktop에 있음).

desktop 배치·점검:

```sh
scp target/x86_64-pc-windows-msvc/release/{win-filetool,filetool-cli}.exe desktop:C:/Users/user/dev/win-filetool-builds/1a/
ssh desktop "C:\Users\user\dev\win-filetool-builds\1a\filetool-cli.exe selftest C:\Users\user\dev\win-filetool-builds\1a\selftest"
```

GUI는 ssh 세션(세션 0)이 아니라 로그인한 사용자 세션에서 떠야 보인다. 원격 실행은 `schtasks /create … /it` +
`schtasks /run`으로 사용자 세션에서 띄웠다.
