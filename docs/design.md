# win-filetool 설계

상태: 1a(브랜치 `salarycrew/stage1a`) + 1b(`salarycrew/stage1b`) + 1c 구현(`salarycrew/stage1c`). **스택 확정: Tauri 2** (Rust 코어 + 웹 UI).

## 단계 나누기

| 단계 | 내용 | 상태 |
|---|---|---|
| **1a** | Tauri 2 뼈대(한국어 UI, 끌어다 놓기, 저장 위치), 맥↔윈도 한글 파일명 NFD→NFC(폴더·zip), PDF 페이지 작업(합치기·나누기·회전·삭제·순서 변경) | 완료 |
| **1b** | 변환: 이미지(JPG/PNG/WebP/BMP/TIFF/GIF)·HEIC(ffmpeg), 영상·소리 프리셋(ffmpeg), 문서·Pages/Numbers/Keynote(LibreOffice), 감시 폴더, 도구 설정 화면 | 완료(아래) |
| **1c** | PDF 압축, 이미지↔PDF, 설치 파일(NSIS, 사용자별)·아이콘, 감시 폴더용 윈도 시작 시 실행 + 트레이, 한 번만 실행 | 완료(아래) |
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
  -- 1c --
  pdfcompress.rs PDF 압축: 이미지가 찍히는 크기(콘텐츠 q/Q/cm/Do 추적)로 DPI를 구해 줄이고 JPEG로 다시 인코딩, prune + 객체 스트림
  img2pdf.rs  이미지 → PDF: JPEG는 바이트 그대로(DCTDecode), EXIF 회전은 그리기 행렬로. 나머지는 Flate(+SMask), HEIC는 1b 경로 → JPEG
  pdfrender.rs PDF → 이미지: PDFium(pdfium-render, 실행 중에 pdfium.dll 불러옴). 들어 있는 이미지 꺼내기(lopdf, PDFium 불필요)
  autostart.rs 윈도 시작 시 실행: HKCU Run 값 만들기·읽기·지우기(winreg, Windows에서만)
crates/cli    filetool-cli.exe — 같은 코어를 명령줄로(tools/image/media/doc/watch 포함). `selftest <빈 폴더> [--apple <폴더>]`로 실제 디스크에서 전 작업 점검
src-tauri     win-filetool.exe — Tauri 2 앱. 명령마다 spawn_blocking으로 돌려 창이 멈추지 않음. 1c: 트레이, 한 번만 실행(single-instance 플러그인), `--autostart`.
              오류는 코어의 한국어 메시지를 그대로 화면 결과 칸에 표시
src/, index.html  Vite + TypeScript(프레임워크 없음). 창 제목 "파일 도구"
```

원칙: 원본은 바꾸지 않는다(폴더 이름 바꾸기는 예외이며 미리보기 후 적용). 출력은 새 파일로만 쓰고,
같은 이름이 있으면 `(1)`을 붙인다. NFC 이름 바꾸기에서 같은 NFC 이름이 이미 있으면 건너뛰고 보고한다.

## 의존성

- `unicode-normalization`, `walkdir`, `zip`(deflate만), `encoding_rs`(CP949 이름), `lopdf`(기본 기능 끔), `thiserror`, `serde`
- 앱: `tauri` 2, `tauri-plugin-dialog`(파일·폴더 고르기)
- 1b: `image`(jpeg/png/webp/bmp/tiff/gif 디코더·인코더), `webp`+`libwebp-sys`(WebP 손실 압축, `sse41` 기능 켬 — clang-cl 크로스 빌드에 필요), `serde_json`
- 외부 도구(동봉하지 않음): ffmpeg/ffprobe, LibreOffice
- 1c: `pdfium-render` 0.9.4(MIT/Apache-2.0, 기능 `pdfium_7881`·`image_025`·`thread_safe`) + 동봉하는 `pdfium.dll`(아래), `winreg`(MIT, Windows에서만), `tauri` 기능 `tray-icon`, `tauri-plugin-single-instance` 2

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
bash scripts/fetch-pdfium.sh         # 1c: PDFium(third_party/, sha256 고정)
bash scripts/fetch-nsis.sh           # 1c: makensis(~/.local/opt/nsis-3.10)
bash scripts/build-windows.sh        # = npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
                                     #   + cargo xwin build --release -p filetool-cli + 휴대용 묶음
# → dist-win/파일도구_0.1.0_x64-setup.exe, dist-win/win-filetool-0.1.0-portable/(.zip)
cargo test -p filetool-core          # 코어 테스트(Linux)
```

1c부터 설치 파일(NSIS)도 만든다(아래 "1c > 설치 파일"). 휴대용 묶음은 exe + pdfium.dll로 실행된다(WebView2 런타임 필요 — Windows 11에는 기본으로 있음).

desktop 배치·점검:

```sh
scp -r dist-win/* desktop:C:/Users/user/dev/win-filetool-builds/1c/        # 설치 파일 + 휴대용 묶음(portable\)
ssh desktop "C:\Users\user\dev\win-filetool-builds\1c\portable\filetool-cli.exe selftest C:\Users\user\dev\win-filetool-builds\1c\selftest --soffice C:\Users\user\dev\win-filetool-builds\tools\lo\program --apple C:\Users\user\dev\win-filetool-builds\1b\apple"
```

GUI는 ssh 세션(세션 0)이 아니라 로그인한 사용자 세션에서 떠야 보인다. 원격 실행은 `schtasks /create … /it` +
`schtasks /run`으로 사용자 세션에서 띄웠다.

Apple 형식 점검 파일: LibreOffice 저장소의 가져오기 테스트 자료(MPL-2.0) `writerperfect/qa/unit/data/{writer,calc,impress}/libetonyek/pass/`의
`Pages_5.pages`, `Pages_4.pages`, `Numbers_2.numbers`, `Keynote_6.key`. 저장소에는 넣지 않고 점검 때만 내려받는다.

## 1c

### PDF 압축 (lopdf + image, 외부 도구 없음)

프리셋: **화면용** 96 DPI·JPEG 품질 55 / **보통** 150 DPI·70 / **인쇄용** 300 DPI·85.

1. 쪽마다 콘텐츠 스트림을 풀어 `q`/`Q`/`cm`/`Do`로 CTM을 따라가며(Form XObject 안까지, 깊이 8) 이미지가 몇 pt로 찍히는지 구한다.
   여러 번 쓰이면 가장 크게 찍힌 크기를 기준으로 한다(그 자리에 필요한 해상도는 남김). 찾지 못한 이미지(주석·패턴 등)는 줄이지 않고 품질만 바꾼다.
2. 해상도가 프리셋 DPI보다 10% 넘게 높으면 그 DPI로 줄이고(Lanczos3), JPEG로 다시 인코딩한다. **결과가 원래 스트림보다 크면 그 이미지는 그대로 둔다.**
3. 건드리지 않는 것: 글자·벡터(콘텐츠 스트림·글꼴은 바이트 그대로 — 테스트로 확인), 마스크(ImageMask, 다른 이미지의 SMask/Mask), 색 키 마스크(`/Mask` 배열),
   8비트가 아닌 이미지, CMYK·Indexed·Lab 등(RGB·흑백·ICCBased N=1/3만), `/Decode`가 있는 이미지, JPX/JBIG2/CCITT 등, 4 KB 미만.
4. 쓰지 않는 객체 삭제(prune), 압축 안 된 스트림 Flate, **객체 스트림 + xref 스트림**으로 저장(PDF 1.5 이상).
5. 결과가 원본보다 작지 않으면 새 파일을 만들지 않고 "더 줄지 않아 새 파일을 만들지 않았습니다(원본 그대로)"라고 알린다. 출력 이름 `이름_압축.pdf`.
6. 암호가 걸린 PDF(열기 암호, 권한 암호만 있는 것 모두)는 "암호가 걸린 PDF는 압축할 수 없습니다…"로 끝내고 아무것도 쓰지 않는다.

qpdf·Ghostscript를 쓰지 않은 이유: 동봉하면 크기·라이선스(Ghostscript AGPL) 부담이 있고, 필요한 것은 "사진만 줄이기"라 lopdf로 충분했다.

### 이미지 → PDF

- 한 쪽에 한 장, 목록 순서(화면에서 ▲▼로 바꿈). 출력 `첫이미지.pdf`(한 장) / `첫이미지_외N장.pdf`.
- **JPEG(baseline/progressive, RGB·흑백)는 다시 인코딩하지 않고 그대로** 넣는다. EXIF 회전(1~8, 뒤집기 포함)은 픽셀을 돌리지 않고 그리기 행렬(`cm`)로 반영한다.
  8가지 값 모두 PDFium으로 그려 사분면 색으로 확인(테스트).
- CMYK JPEG·PNG·WebP·BMP·TIFF·GIF: 풀어서(EXIF 회전 적용) Flate 무손실, 투명도는 SMask. HEIC: 1b 경로(ffmpeg, grid·irot)로 푼 뒤 JPEG 품질 92(무손실로 넣으면 원본보다 몇 배 커짐).
- 쪽 크기: **이미지 크기 그대로**(96 DPI 기준 1px = 0.75pt, + 여백) / **A4 세로·가로에 맞춤**(여백 안에 비율 유지, 가운데; 작은 그림은 키워서 맞춤). 여백 mm.

### PDF → 이미지 (렌더러 선택)

**PDFium**(Chrome의 PDF 엔진, BSD-3-Clause)을 `pdfium-render` 크레이트로 실행 중에 불러온다. 바이너리는 bblanchon/pdfium-binaries
`chromium/7881`(pdfium-render 0.9.4 기본 API와 같은 버전, `scripts/fetch-pdfium.sh`가 sha256 확인 후 `third_party/`에 받음 — 저장소에는 넣지 않음).

- 관리자 권한 불필요: `pdfium.dll`(약 7 MB)을 exe 옆에 둔다. **설치 파일과 휴대용 묶음 모두 같은 위치에 넣는다**(일관되게 동봉). 라이선스 파일은 `licenses\pdfium\`.
- 찾는 순서: 환경 변수 `WIN_FILETOOL_PDFIUM`(파일/폴더) → exe 옆 → exe 옆 `pdfium\`, `tools\pdfium\(bin)`. 없으면 한국어 안내.
- 크기: 쪽 크기(pt, /Rotate 반영) × DPI / 72. 예: A4 595×842pt, 150 DPI → 1240×1754(PDFium이 반올림). PNG/JPG, 쪽 범위(`1-3,5`) 지정. 결과 폴더 `이름_쪽이미지\이름_p01.png`.
- 다른 후보: MuPDF(AGPL — 앱 전체 공개 의무), poppler(GPL), Ghostscript(AGPL), pdf.js(웹뷰 안에서 렌더 — CLI·감시와 공유 불가). 허용적 라이선스이고 품질이 검증된 PDFium을 골랐다.
- **들어 있는 사진만 꺼내기**: lopdf로 이미지 객체를 그대로 저장(JPEG는 바이트 그대로 `.jpg`, 8비트 Flate RGB/흑백은 `.png`, 그 밖은 건너뛰고 개수만 알림). PDFium 불필요.

### 설치 파일·아이콘

- 아이콘 원본은 `assets/app-icon.svg` 하나(파란 바탕 + 흰 문서 + 주황 화살표, 16px에서도 보이게 굵은 도형만). `scripts/make-icons.sh`(= `tauri icon`)가
  `src-tauri/icons/`(icon.ico 16·24·32·48·64·256 포함)를 만든다. 창·작업 표시줄·트레이·설치 파일이 같은 아이콘을 쓴다.
- **사용자별 NSIS 설치 파일**(Tauri bundler, `installMode: currentUser`, 한국어, 언어 선택 없음): 관리자 권한·UAC 없이
  `%LOCALAPPDATA%\파일 도구\`에 설치, 시작 메뉴 바로가기 "파일 도구", [설정 > 앱]에 제거 항목(HKCU), 제거 프로그램 `uninstall.exe`.
  제거할 때 훅(`src-tauri/windows/installer-hooks.nsh`)이 HKCU Run 값도 지운다. 조용히: `파일도구_0.1.0_x64-setup.exe /S`, 제거 `uninstall.exe /S`.
- `makensis`는 K12에 sudo 없이: `scripts/fetch-nsis.sh`가 Ubuntu `nsis`/`nsis-common` 패키지를 `apt-get download`로 받아 `~/.local/opt/nsis-3.10`에 풀고
  `NSISDIR`을 맞춰 주는 감싸개를 만든다. Tauri의 Linux→Windows NSIS 묶기가 cargo-xwin과 함께 그대로 동작했다.
- 휴대용 묶음도 계속 만든다: `win-filetool-<버전>-portable\`(+ zip) = `win-filetool.exe`, `filetool-cli.exe`, `pdfium.dll`, `licenses\`.
- **서명하지 않음**: 처음 실행할 때 Windows SmartScreen이 "Windows의 PC 보호" 창을 띄울 수 있다 → [추가 정보] → [실행]. 내려받은 zip은
  파일 속성에서 [차단 해제]를 먼저 하면 경고가 줄어든다. 코드 서명 인증서를 쓰면 없어진다(1c 범위 밖).

### 윈도 시작 시 실행 + 트레이

- [설정] 탭 "윈도우 시작 시 실행"(기본 꺼짐): `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`에 값 `win-filetool` =
  `"<exe 경로>" --autostart`(관리자 권한 불필요). 끄면 값을 지운다. 상태는 레지스트리 값 자체(설정 파일에 따로 적지 않음). 앱을 옮겨 경로가 다르면 화면에 알리고 껐다 켜면 고쳐진다.
  CLI: `filetool-cli autostart on|off|status`.
- `--autostart`로 시작하면 창을 띄우지 않고(창은 숨긴 채 만들어짐) 트레이 아이콘만, 켜진 감시 규칙을 돌린다.
- 트레이 메뉴: **열기 / 감시 일시정지(체크) / 끝내기**, 아이콘 왼쪽 클릭 = 열기. 툴팁에 감시 상태. [감시 폴더] 화면의 "감시 일시정지"와 같은 값.
- 창 닫기(X): 켜진 감시 규칙이 있으면 창만 숨기고 트레이에 남는다. 없으면 앱이 끝난다.
- **한 번만 실행**(tauri-plugin-single-instance): 두 번째 실행은 기존 창을 앞으로 가져오고 끝난다(`--autostart`로 온 두 번째 실행은 조용히 끝남).
  1b의 `watch.lock`은 그대로 둔다(같은 설정 폴더를 쓰는 다른 프로세스, 예: 설정 폴더를 바꾼 시험 실행, 에서도 감시는 한 곳만).

### 설정이 있는 곳

| 무엇 | 위치 |
|---|---|
| 설정(도구 경로, 감시 규칙) | `%APPDATA%\kr.salarycrew.winfiletool\settings.json` |
| 감시 기록·잠금 | 같은 폴더 `watch-state.json`, `watch.lock` |
| 윈도 시작 시 실행 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `win-filetool` |
| 웹뷰 데이터 | `%LOCALAPPDATA%\kr.salarycrew.winfiletool\` |
| 작업 임시 파일 | `%TEMP%\win-filetool\…`(작업이 끝나면 지움) |
| 설치 위치(설치 파일) | `%LOCALAPPDATA%\파일 도구\` (win-filetool.exe, pdfium.dll, licenses, uninstall.exe) |

제거 프로그램은 앱 파일·시작 메뉴·Run 값을 지운다. 설정 폴더는 Tauri 제거 화면의 "앱 데이터 삭제"를 고르면 지워진다(조용히 제거할 때는 남음).
