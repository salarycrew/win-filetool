# win-filetool 설계

상태: 1a(브랜치 `salarycrew/stage1a`) + 1b 구현(브랜치 `salarycrew/stage1b`). **스택 확정: Tauri 2** (Rust 코어 + 웹 UI).

## 단계 나누기

| 단계 | 내용 | 상태 |
|---|---|---|
| **1a** | Tauri 2 뼈대(한국어 UI, 끌어다 놓기, 저장 위치), 맥↔윈도 한글 파일명 NFD→NFC(폴더·zip), PDF 페이지 작업(합치기·나누기·회전·삭제·순서 변경) | 완료 |
| **1b** | 변환: 이미지(JPG/PNG/WebP/BMP/TIFF/GIF)·HEIC(ffmpeg), 영상·소리 프리셋(ffmpeg), 문서·Pages/Numbers/Keynote(LibreOffice), 감시 폴더, 도구 설정 화면 | 완료(아래) |
| 1c | PDF 압축, 이미지↔PDF, 설치 파일(NSIS)·아이콘, 감시 폴더용 윈도 시작 시 자동 실행 | 다음 |
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
  samples.rs  점검용 샘플(NFD 폴더, 맥식 zip, N쪽 PDF, 사분면 색 그림, EXIF JPEG, grid HEIC, 시험 영상) 생성
  -- 1b --
  tools.rs    ffmpeg/ffprobe/soffice 찾기(아래 "외부 도구") + 버전 확인 + 없을 때 한국어 안내
  process.rs  외부 도구 실행: 셸 없이 argv, 작업별 시간 제한, 취소 시 자식 트리 종료(Windows taskkill /T),
              콘솔 창 안 띄움(CREATE_NO_WINDOW), stdout 줄 단위 콜백(ffmpeg -progress)
  imageconv.rs  image 크레이트(+WebP 손실 압축은 libwebp). EXIF 회전을 픽셀에 적용, 품질·긴 변 제한, 투명→흰 바탕(JPG)
  heif.rs     HEIF 상자 해석(pitm/iinf/iref/iprp/ipma/iloc/idat): grid 배치·irot/imir·Exif 항목. 점검용 grid HEIC 생성기
  exif.rs     EXIF(TIFF) 촬영 날짜·회전 값 읽기, 점검용 EXIF 생성
  media.rs    ffmpeg 프리셋 argv, -progress 해석, ffprobe 길이
  docs.rs     LibreOffice --convert-to argv, 작업마다 개인 프로필(-env:UserInstallation=file:///…), 임시 출력 → 겹치지 않는 이름으로 이동
  convert.rs  파일 하나 변환 진입점(Op = Image | Media | Doc). CLI·앱·감시 폴더가 같이 씀
  watch.rs    감시 폴더 규칙·크기 안정 대기·상태 파일
  settings.rs 설정(도구 경로, 감시 규칙) JSON 저장(임시 파일 → 바꿔치기)
  tempdir.rs  작업용 임시 폴더(%TEMP%\win-filetool\…, 끝나면 지움)
crates/cli    filetool-cli.exe — 같은 코어를 명령줄로(tools/image/media/doc/watch 포함). `selftest <빈 폴더> [--apple <폴더>]`로 실제 디스크에서 전 작업 점검
src-tauri     win-filetool.exe — Tauri 2 앱. 명령마다 spawn_blocking으로 돌려 창이 멈추지 않음.
              오류는 코어의 한국어 메시지를 그대로 화면 결과 칸에 표시
src/, index.html  Vite + TypeScript(프레임워크 없음). 창 제목 "파일 도구"
```

원칙: 원본은 바꾸지 않는다(폴더 이름 바꾸기는 예외이며 미리보기 후 적용). 출력은 새 파일로만 쓰고,
같은 이름이 있으면 `(1)`을 붙인다. NFC 이름 바꾸기에서 같은 NFC 이름이 이미 있으면 건너뛰고 보고한다.

## 의존성

- `unicode-normalization`, `walkdir`, `zip`(deflate만), `encoding_rs`(CP949 이름), `lopdf`(기본 기능 끔), `thiserror`, `serde`
- 앱: `tauri` 2, `tauri-plugin-dialog`(파일·폴더 고르기)
- 1b: `image`(jpeg/png/webp/bmp/tiff/gif 디코더·인코더), `webp`+`libwebp-sys`(WebP 손실 압축, `sse41` 기능 켬 — clang-cl 크로스 빌드에 필요), `serde_json`
- 외부 도구(동봉하지 않음): ffmpeg/ffprobe, LibreOffice. 1c 예정: PDF 압축(qpdf 또는 lopdf+이미지 재압축)

## 1b 변환

| 묶음 | 내용 | 방법 |
|---|---|---|
| 이미지 | JPG·PNG·WebP·BMP·TIFF·GIF(첫 장면) 서로 변환, 품질(JPG·WebP), 긴 변 최대 | Rust `image` (+libwebp) |
| HEIC/HEIF | → JPG/PNG/… (같은 이미지 옵션) | ffmpeg로 풀기. grid(아이폰 512px 타일) 이미지는 앱이 HEIF 구조를 읽어 타일별로 ffmpeg에 풀게 한 뒤 이어 붙이고, 출력 크기로 자르고, irot/imir를 적용. ffmpeg 8.0.1은 grid를 스스로 합치지 않고 첫 타일만 내기 때문 |
| 영상·소리 | MP4(H.264/AAC), 메신저용 줄이기(목표 MB·최대 세로 해상도, 길이로 비트레이트 계산), 소리만 MP3/M4A, 짧은 GIF(팔레트 생성) | ffmpeg 프리셋, `-progress pipe:1` 진행률, `-n`(덮어쓰지 않음) |
| 문서 | DOCX/XLSX/PPTX/ODF/RTF/TXT/HWP(한글 97만)/Pages/Numbers/Keynote → PDF, Pages→DOCX, Numbers→XLSX, Keynote→PPTX, ODF→Office | LibreOffice headless `--convert-to`(Apple 형식은 LibreOffice의 libetonyek 가져오기) |

보존하는 것(사진): EXIF 블록 전체(촬영 날짜 포함)를 JPG·PNG 출력에 옮긴다. 회전은 픽셀에 적용하고 EXIF 회전 값은 1로 바꾼다.
HEIC는 HEIF 안의 Exif 항목을 옮긴다. WebP·BMP·TIFF·GIF 출력에는 EXIF를 넣지 않는다. 모든 변환 결과의 수정 시각은 원본과 같게 맞춘다.
ICC 색 프로필·XMP는 옮기지 않는다.

원본은 바꾸지 않고, 같은 이름이 있으면 `이름 (1)`. 메신저용은 `이름_메신저.mp4`. 실패하거나 취소하면 만들던 출력은 지운다.

## 외부 도구 찾기 (ffmpeg, LibreOffice)

앱에 넣지 않는다(ffmpeg GPL 빌드·LibreOffice 크기). [도구 설정] 화면에서 찾은 위치·버전(실제로 `-version`/`--version` 실행)을 보여 주고,
없으면 한국어 안내와 내려받을 곳을 보여 준다. 1b는 자동 설치하지 않는다. 찾는 순서:

1. 사용자가 [경로 지정]으로 고른 경로(설정 `%APPDATA%\kr.salarycrew.winfiletool\settings.json`, 실행 파일 또는 그 폴더/bin/program)
2. PATH
3. ffmpeg: `%LOCALAPPDATA%\Microsoft\WinGet\Links`, `…\WinGet\Packages\*ffmpeg*\(bin|*\bin)`, `Program Files\ffmpeg\bin`, `C:\ffmpeg\bin`, chocolatey, scoop
   LibreOffice: `Program Files(\x86)\LibreOffice\program`, LibreOffice Portable(`%USERPROFILE%`·바탕 화면·문서·다운로드·PortableApps 아래 `LibreOfficePortable\App\libreoffice\program`), `%USERPROFILE%\LibreOffice\program`, scoop
4. 앱 옆 `tools\<이름>\(bin|program)` (휴대용 배치)

ffprobe는 찾은 ffmpeg와 같은 폴더를 먼저 본다. 관리자 권한 없이 LibreOffice를 쓰려면 설치 파일을 폴더에 풀면 된다
(desktop에서 확인: `msiexec /a LibreOffice_26.2.6_Win_x86-64.msi /qn TARGETDIR=<폴더>` → `<폴더>\program\soffice.exe`, 권한 상승 없이 성공).

외부 도구는 셸을 거치지 않고 argv로만 실행한다. 시간 제한: HEIC 2~5분, 문서 5분, 영상 4시간. 취소하면 자식 프로세스 트리를 끝낸다.
LibreOffice는 작업마다 `%TEMP%\win-filetool\lo-…\profile`을 개인 프로필로 써서 사용자의 LibreOffice 설정을 건드리지 않고, 동시에 돌려도 충돌하지 않는다.

## 감시 폴더

규칙 = 폴더 + 변환(사진→JPG/PNG/WebP, 영상→MP4/메신저용/MP3, 문서→PDF) + 출력 하위 폴더(기본 `변환됨`) + 켜기/끄기. 설정 파일에 저장되어 다시 켜도 남는다.
앱이 열려 있는 동안 백그라운드 스레드가 2초마다 폴더 맨 위 파일을 살핀다(하위 폴더는 보지 않음 — 출력이 다시 입력이 되지 않음).
파일 시스템 알림(`notify`) 대신 폴링을 골랐다: 네트워크 드라이브·USB에서도 같게 동작하고, 복사가 끝났는지는 어차피 크기 안정으로 판단해야 하기 때문.

- 복사 중: 크기·수정 시각이 3초 동안 그대로이고, (Windows) 읽기 공유만 허용해 열어 볼 수 있을 때(다른 프로그램이 쓰는 중이면 실패) 처리
- 건너뜀: `.`·`~$`로 시작, `.tmp/.part/.crdownload/.download` 등, 규칙에 맞지 않는 형식
- 다시 처리하지 않음: `watch-state.json`에 파일 이름+크기+수정 시각과 결과, 만든 출력 이름을 적는다. 실패도 적어서 계속 다시 시도하지 않고, 내용이 바뀌면(크기·시각) 다시 처리
- 결과는 [감시 폴더] 화면 기록에 실시간으로 표시(최근 200건)
- 창을 여러 개 열어도 감시는 한 프로세스만: 설정 폴더의 `watch.lock`을 잡은 창만 규칙을 돌린다(그 창이 닫히면 다른 창이 이어받음)

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

설치 파일(NSIS)은 1a·1b에서 만들지 않는다(`makensis` 없음, `--no-bundle`). 휴대용 exe 하나로 실행된다(WebView2 런타임 필요 — desktop에 있음).

desktop 배치·점검:

```sh
scp target/x86_64-pc-windows-msvc/release/{win-filetool,filetool-cli}.exe desktop:C:/Users/user/dev/win-filetool-builds/1b/
ssh desktop "C:\Users\user\dev\win-filetool-builds\1b\filetool-cli.exe selftest C:\Users\user\dev\win-filetool-builds\1b\selftest --soffice C:\Users\user\dev\win-filetool-builds\tools\lo\program --apple C:\Users\user\dev\win-filetool-builds\1b\apple"
```

GUI는 ssh 세션(세션 0)이 아니라 로그인한 사용자 세션에서 떠야 보인다. 원격 실행은 `schtasks /create … /it` +
`schtasks /run`으로 사용자 세션에서 띄웠다.

Apple 형식 점검 파일: LibreOffice 저장소의 가져오기 테스트 자료(MPL-2.0) `writerperfect/qa/unit/data/{writer,calc,impress}/libetonyek/pass/`의
`Pages_5.pages`, `Pages_4.pages`, `Numbers_2.numbers`, `Keynote_6.key`. 저장소에는 넣지 않고 점검 때만 내려받는다.
