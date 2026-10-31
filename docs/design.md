# win-filetool 설계

상태: 1a(브랜치 `salarycrew/stage1a`) + 1b(`salarycrew/stage1b`) + 1c(`salarycrew/stage1c`, main 병합) + 2a 구현(`salarycrew/stage2a`) + 2b-core(`salarycrew/stage2b`) + 2c(`salarycrew/stage2c`). **스택 확정: Tauri 2** (Rust 코어 + 웹 UI).

## 단계 나누기

| 단계 | 내용 | 상태 |
|---|---|---|
| **1a** | Tauri 2 뼈대(한국어 UI, 끌어다 놓기, 저장 위치), 맥↔윈도 한글 파일명 NFD→NFC(폴더·zip), PDF 페이지 작업(합치기·나누기·회전·삭제·순서 변경) | 완료 |
| **1b** | 변환: 이미지(JPG/PNG/WebP/BMP/TIFF/GIF)·HEIC(ffmpeg), 영상·소리 프리셋(ffmpeg), 문서·Pages/Numbers/Keynote(LibreOffice), 감시 폴더, 도구 설정 화면 | 완료(아래) |
| **1c** | PDF 압축, 이미지↔PDF, 설치 파일(NSIS, 사용자별)·아이콘, 감시 폴더용 윈도 시작 시 실행 + 트레이, 한 번만 실행 | 완료(아래) |
| **2a** | Everything식 전체 파일 즉시 검색 — **관리자 권한 없이**: 폴더 병렬 훑기 색인, 저장·불러오기, `ReadDirectoryChangesW` 실시간 갱신, 질의 언어(NFC/NFD·초성), 검색 탭·트레이·전역 단축키, CLI `index`/`search` | 완료(아래 "2a") |
| **2b-core** | NTFS 빠른 색인: `$MFT` 직접 읽기(`MftSource`), USN 저널 따라잡기·따라가기, 색인 형식 v2 — **이미 관리자 권한으로 실행됐을 때만 자동**, CLI `--source`·`usn-catchup` | 완료(아래 "2b-core") |
| **2c** | 일반 사용자가 빠른 길을 쓰는 방법 = **선택 설치 Windows 서비스**(사용자 결정 10-31, 방안 C): `filetool-svc.exe`가 MFT·저널을 맡고 앱·CLI는 이름 있는 파이프로 묻는다. 앱 설치는 그대로 사용자별·관리자 불필요, 서비스는 [설정]에서 UAC 1번으로 설치/제거 | 완료(아래 "2c") |

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
  -- 2a --
  search/index.rs   메모리 색인(이름 창고 + 부모 번호, 열별 배열), 지운 표시·정리, 루트 바꿔 끼우기, 파일 저장·읽기
  search/scan.rs    IndexSource 트레잇 + DirWalkSource(FindFirstFileExW 병렬 훑기), 제외 패턴, 고정 드라이브 찾기
  search/live.rs    실시간 변경 알림(Windows: ReadDirectoryChangesW 직접, 그 밖: notify/inotify)
  search/query.rs   질의 언어 해석·컴파일·실행(여러 스레드), 정렬, 결과 줄
  search/service.rs 불러오기 → 감시 → 배경 스캔 → 0.3초 모아 반영 → 저장. 앱·CLI 공용
  search/text.rs    비교 키(NFC + 소문자), 초성, 와일드카드
  -- 2c --
  search/svcproto.rs 서비스 파이프 프로토콜(틀·봉투·버전·한도), 요청 처리(Handler/Session), 가시성 규칙(Visibility/ResultFilter), 클라이언트(플랫폼 무관, 시험 가능)
  search/svcpipe.rs  Windows: 파이프 서버(DACL·원격 거부·첫 인스턴스), 호출자 확인(흉내 내기 토큰), 클라이언트(서버 pid = SCM 서비스 pid 확인, 겹친 I/O 시간 제한), SCM 조회
crates/svc    filetool-svc.exe — 빠른 검색 서비스(install/uninstall/status/run). 코어의 SearchService + 파이프 서버
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
- 2a: `memchr`(이름 훑기 memmem), `chrono`(dm: 지역 날짜), `windows-sys` 0.61(FindFirstFileExW·ReadDirectoryChangesW·드라이브 종류, Windows에서만),
  `notify` 8(Linux·시험용 실시간 알림, Windows 빌드에는 안 들어감), `tauri-plugin-global-shortcut` 2(전역 단축키)
- 2c: 새 외부 크레이트 없음. `windows-sys` 기능 추가(Pipes·Services·Threading·Security_Authorization, 서비스는 Authentication_Identity(LSA), 앱은 UI_Shell(ShellExecuteExW)), 서비스·CLI에 `chrono`(기록 시각)

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
# → dist-win/파일도구_0.2.1_x64-setup.exe, dist-win/win-filetool-0.2.1-portable/(.zip)
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
  제거할 때 훅(`src-tauri/windows/installer-hooks.nsh`)이 HKCU Run 값도 지운다. 조용히: `파일도구_0.2.1_x64-setup.exe /S`, 제거 `uninstall.exe /S`.
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
| 설정(도구 경로, 감시 규칙, 2a 검색 루트·제외·단축키) | `%APPDATA%\kr.salarycrew.winfiletool\settings.json` |
| 검색 색인 | `%LOCALAPPDATA%\kr.salarycrew.winfiletool\index\index-v2.bin` (2b부터. 2a의 `index-v1.bin`은 읽은 뒤 v2를 저장하면 지움. 시험용으로 `WIN_FILETOOL_INDEX_DIR`로 바꿀 수 있음) |
| 감시 기록·잠금 | 같은 폴더 `watch-state.json`, `watch.lock` |
| 윈도 시작 시 실행 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `win-filetool` |
| 웹뷰 데이터 | `%LOCALAPPDATA%\kr.salarycrew.winfiletool\` |
| 작업 임시 파일 | `%TEMP%\win-filetool\…`(작업이 끝나면 지움) |
| 설치 위치(설치 파일) | `%LOCALAPPDATA%\파일 도구\` (win-filetool.exe, pdfium.dll, licenses, uninstall.exe, 2c부터 filetool-svc.exe — 서비스 설치 단계가 여기서 Program Files로 복사) |
| 2c 서비스 실행 파일 | `%ProgramFiles%\파일 도구 서비스\filetool-svc.exe` (SYSTEM·관리자 전체, Users 읽기·실행만) |
| 2c 서비스 색인·기록 | `%ProgramData%\kr.salarycrew.winfiletool\index\index-v2.bin`, `service.log`(1 MB 넘으면 `service.old.log`) — SYSTEM·관리자만 |

제거 프로그램은 앱 파일·시작 메뉴·Run 값을 지운다. 설정 폴더는 Tauri 제거 화면의 "앱 데이터 삭제"를 고르면 지워진다(조용히 제거할 때는 남음).

## 2a — 전체 파일 즉시 검색 (관리자 권한 없이)

### 왜 관리자 권한 없는 길을 먼저

Everything처럼 NTFS MFT를 통째로 읽는 길(`FSCTL_ENUM_USN_DATA`, USN 저널)은 볼륨 핸들(`\\.\C:`)이 필요하고, 그 핸들은 관리자 권한이 있어야 열린다.
desktop은 원격에서 권한 상승(UAC)을 할 수 없고, 많은 사용자도 관리자로 실행하지 않는다. 그래서 2a는 **검색 기능 전체**(색인·저장·실시간 갱신·질의·UI)를
관리자 권한 없이 만들고, MFT/USN은 같은 자리(`IndexSource`)에 꽂을 수 있는 빠른 길로 2b에 남긴다.

### 색인 (`search/index.rs`)

- 항목마다 `PathBuf`를 두지 않는다. **이름 창고**(모든 이름을 이어 붙인 `Vec<u8>`) + 항목별 열 배열:
  이름 위치 u32·길이 u16, 비교 키 위치·길이, 부모 u32, 첫 자식·다음 형제 u32(자식 찾기·지우기), 크기 u64, 수정 시각 i64(유닉스 초), 종류 u8(폴더·지움·루트).
- **비교 키** = NFC + 소문자. 맥에서 온 NFD 이름과 NFC 질의가 같은 키가 된다(질의도 같은 변환). 키는 파일에 저장하지 않고 읽을 때 다시 만든다.
- 부모 번호는 언제나 자식보다 작다(스캔·실시간 추가 모두 부모를 먼저 넣음) → 번호 순서로 한 번 돌면 폴더 경로 캐시를 만든다(경로 낱말·폴더 정렬용, 내용이 바뀔 때만 다시).
- 지운 항목은 표시만 하고 저장할 때·루트를 바꿔 끼울 때 정리(번호 다시 매김 → 세대 `generation` 증가, 화면은 다시 검색).
- **이름 순위**: 스캔·불러오기가 끝나면 배경에서 전체를 이름순으로 한 번 정렬해 순위(u32)를 만들어 둔다. 큰 결과(수십만 개)의 이름 정렬이 문자열 비교 대신 정수 비교가 된다(77만 개 456 ms → 56 ms).

**파일 형식 v1** (`index-v1.bin`, 리틀 엔디언 — 2b에서 v2로 올림, 아래 "2b-core"): `WFTIDX01` · 버전 u32 · 만든 시각 i64 · 루트 수 u32 · 루트마다(경로, 번호, 마지막 스캔, 통계 6개) ·
항목 수 u32 · parent[u32] · flags[u8] · size[u64] · mtime[i64] · name_len[u16] · 이름 바이트 · **FNV-1a 64 검사합**.
쓸 때는 `index-v1.bin.tmp`에 쓰고 `sync` 후 바꿔치기(중간에 끊겨도 옛 파일이 남음). 형식·버전·검사합이 다르면 버리고 새로 스캔한다.
형식이 바뀌면 파일 이름의 버전도 올린다(`index-v2.bin`).

### 스캐너 (`search/scan.rs`)

- 트레잇 `IndexSource::scan(dir, dir_id, target, opts, ctl)` — 2a의 `DirWalkSource`, 2b의 MFT 원본이 같은 자리에.
- `DirWalkSource`: 작업 큐 + 스레드(CPU 수, 최대 8)가 폴더마다 목록을 읽는다. Windows는 `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)`(8.3 이름 안 읽음, 큰 버퍼), `\\?\` 긴 경로.
  목록 하나를 읽으면 쓰기 잠금을 잠깐 잡고 그 폴더의 항목을 넣는다 → **스캔 중에도 읽기 잠금으로 부분 결과 검색**.
- 연결 지점: reparse 태그가 **이름 대리**(정션·심볼릭 링크·마운트 지점, `0x20000000`)면 항목으로만 넣고 따라가지 않는다(고리 방지, 수를 셈).
  OneDrive 자리 표시 폴더처럼 이름 대리가 아닌 reparse는 들어간다. 열 수 없는 폴더(권한 없음)는 건너뛰고 센다.
- 제외 패턴: `\`가 있으면 전체 경로(와일드카드, 대소문자·NFC 무시), 없으면 폴더 이름. 기본 `$Recycle.Bin`, `System Volume Information`, `?:\Windows\WinSxS`(설정에서 보이고 고칠 수 있음).
  색인 폴더 자체도 넣지 않는다.
- 루트: 설정이 비어 있으면 `GetLogicalDrives` + `GetDriveTypeW == DRIVE_FIXED`인 드라이브 전부.

### 실시간 갱신 (`search/live.rs`, `search/service.rs`)

- `notify` 크레이트(8.2)의 Windows 쪽은 버퍼가 넘치면(돌려받은 크기 0) 알림을 **조용히 버리고**, 오류가 나면 감시를 끊는다. "넘치면 그 아래만 다시 훑기"를 하려면
  넘침을 알아야 해서 `ReadDirectoryChangesW`를 직접 쓴다(64 KB 버퍼, 스레드마다 동기 호출, 끝낼 때 `CancelIoEx`).
- 감시 단위: 루트 자체는 하위 없이, **루트 바로 아래 폴더마다 하위 전체**(64개 넘으면 루트 하나를 통째로). 버퍼가 넘치거나 오류가 나면 그 감시 폴더만 다시 훑는다.
  루트 바로 아래에 새 폴더가 생기면 감시를 붙이고, 감시 폴더가 사라지면 그 스레드는 끝난다.
- 알림은 0.3초 모아 같은 경로를 합친 뒤 경로마다 지금 상태를 본다(`symlink_metadata`): 없으면 그 항목과 아래를 지우고, 있으면 크기·시각을 고치거나 새로 넣는다.
  새 폴더면 그 아래를 훑는다. 부모 폴더도 모르면 부모부터. 이름 바꾸기(옛 이름 사라짐 + 새 이름 생김)도 같은 방식.
- 시작 순서: 저장한 색인 불러오기 → 감시 시작 → 루트마다 배경 다시 스캔. 색인에 없던 루트는 살아 있는 색인에 바로 채우고(부분 결과),
  있던 루트는 새 색인에 따로 스캔한 뒤 쓰기 잠금 안에서 통째로 바꿔 끼운다. 그 루트를 스캔하는 동안 온 알림은 모아 두었다가 끝나면 다시 반영한다.
- 저장: 스캔이 모두 끝났을 때, 그 뒤 바뀐 것이 있으면 10분마다, 앱을 끝낼 때(`RunEvent::Exit`).
- 앱: 서비스는 창이 없어도(트레이, `--autostart`) 돈다. 창 닫기(X)는 감시 규칙·윈도 시작 시 실행·전역 단축키 중 하나라도 켜져 있으면 트레이로 숨김.

### 질의 (`search/query.rs`)

낱말 나누기 → 트리(AND/OR/NOT/낱말) → 컴파일(memchr `memmem` Finder, 와일드카드는 가장 긴 글자 조각으로 먼저 거름, AND 안은 싼 조건 먼저) →
항목 배열을 스레드(최대 8)로 나눠 훑기. 문법은 README 표. 정한 것:

- `|`는 AND보다 먼저 묶인다(Everything과 같음). 괄호 묶음은 없음.
- 와일드카드는 이름 **전체**에 맞춘다(`*.jpg`는 `.jpg`로 끝나는 이름). 와일드카드 없는 낱말은 "들어 있음".
- `\`(또는 `/`)가 든 낱말은 전체 경로(루트부터, 구분자 `\`)에 "들어 있음"/와일드카드면 전체 경로에 맞춤.
- `size:`는 파일만(폴더 크기는 모름). 단위는 1024 기준. `dm:`은 지역 시간대, 주는 월요일부터.
- 초성: 낱말이 초성 자음(ㄱ~ㅎ, 쌍자음 포함)으로만 되어 있으면 이름의 한글 음절을 초성으로 바꾼 문자열에서 찾는다(`ㅂㄱㅅ` → `보고서`, `주간보고서`). ASCII 이름은 바로 건너뜀.
- 빈 질의는 결과 없음(전체 목록을 보여 주지 않음).

### 화면

- [검색] 탭이 첫 탭·시작 탭. 입력 0.06초 뒤 검색, 결과 번호 목록은 Rust에 두고 화면은 보이는 줄(±5)만 100줄 단위로 받아 그린다(가상 목록, 줄 높이 24px).
- 머리글 클릭 정렬(이름/폴더/크기/수정한 날짜, 다시 누르면 반대), 결과 수·시간. Enter 열기(`explorer <경로>`), Ctrl+Enter 폴더 열고 선택(`explorer /select,"경로"`),
  오른쪽 클릭 메뉴(열기/폴더 열기/경로 복사/이름 복사). 상태 줄(1초마다): 항목 수, 마지막 스캔, 스캔 중 루트(진행 수), 건너뛴 폴더, 실시간 반영 건수.
  색인 내용이 바뀌면(스캔 진행·실시간 반영) 결과를 다시 검색한다(1초에 한 번까지).
- 트레이 메뉴 [검색 열기], 전역 단축키(`tauri-plugin-global-shortcut`, 기본 꺼짐, 설정에서 키를 눌러 지정, 등록 실패는 한국어로 알림).
- 화면 점검용 인자: `--tab=search --search=<질의>`.

### 잰 값 (2026-10-31)

- K12 합성 100만 항목(평균 깊이 8.7): 만들기 312 ms, 색인 배열 110 MB(RSS 증가 73 MB), 파일 40.7 MB, 저장 85 ms, 읽기 235 ms, 이름 순위 540 ms,
  질의 12종 찾기 p95 3.6~28.3 ms, 이름 정렬 포함 p95 6.8~64.3 ms(77만 개 결과). `crates/core/tests/search_bench.rs`
- desktop `C:\`(관리자 아님, Medium): 1,728,731 항목, 스캔 3.66 초(바로 앞 관리자 ssh 스캔 7.07 초 뒤라 캐시가 따뜻함), 건너뜀 1,169(권한 없음 1,095 + 연결 지점 74),
  색인 88.5 MB, 저장 198 ms, CLI 읽기 198~215 ms, 앱 불러오기(이름 순위 포함) 676~706 ms, 질의 2.7~37.5 ms. 실시간 반영 0.25~0.48 초(CLI), 1~2 초(화면, 상태 1초 주기)


## 2b-core — NTFS 빠른 색인(MFT/USN, 관리자 권한일 때만)

범위: 코어 + CLI + **이미 관리자 권한으로 실행됐을 때 자동으로 쓰기**. 서비스·UAC/runas 화면은 만들지 않았다(아래 "권한 상승 방안"에서 사용자가 고름).

### 길 고르기 (`search/mft.rs` `plan_for`)

루트마다: 루트가 **드라이브 전체**(`C:\`)이고, `GetVolumeInformationW`가 `NTFS`이고, `\\.\C:`를 `GENERIC_READ`로 열 수 있으면(관리자 토큰) MFT.
아니면(폴더 루트, exFAT/FAT USB, 관리자 아님) **조용히 2a 길**(폴더 훑기 + `ReadDirectoryChangesW`). 상태 줄·CLI에 루트마다
"빠른 색인(MFT)" / "일반 색인"과 이유(예: "관리자 권한이 없어 볼륨을 열 수 없습니다…")를 보여 준다.
CLI `--source auto|mft|walk`: `mft`인데 안 되면 한국어 이유를 찍고 **종료 코드 2**(아무것도 스캔·저장하지 않음).

### MFT 읽기 길 (`search/ntfs.rs`)

`FSCTL_ENUM_USN_DATA`는 이름·부모만 주고 크기·수정 시각이 없다(2a 결과와 맞추려면 파일마다 다시 열어야 함). 그래서 **`$MFT`를 직접 읽는다**:

1. 부트 섹터(오프셋 0): 섹터·클러스터 크기, `$MFT` 시작 LCN, FILE 레코드 크기, 64비트 볼륨 일련번호.
2. 레코드 0(`$MFT` 자신)의 이름 없는 `$DATA` 데이터 런 → `$MFT`가 놓인 조각들. `$MFT`가 아주 조각나 `$ATTRIBUTE_LIST`가 있으면 목록의 다른 레코드에서 `$DATA` 조각을 더 모은다.
   레코드 수 = min(data size, initialized size) / 레코드 크기.
3. 조각마다 4 MB씩 차례로 읽고(읽기 스레드 1) 푸는 스레드(CPU 수, 최대 8)가 레코드를 푼다:
   - **fixup**: 512바이트마다 끝 2바이트가 업데이트 시퀀스 번호와 같아야 하고, 배열의 원래 값으로 되돌린다. 안 맞으면(쓰다 끊김) 그 레코드는 버리고 "깨짐"으로 센다.
     이미 되돌린 레코드(`FSCTL_GET_NTFS_FILE_RECORD` 결과)도 받아들인다.
   - 머리의 in-use 비트가 꺼진 레코드(지운 파일)는 건너뛴다. 폴더 비트(0x02) = 폴더.
   - `$STANDARD_INFORMATION`: 수정 시각(FindFirstFile의 `ftLastWriteTime`과 같은 값), 속성. `$REPARSE_POINT`: 태그(이름 대리 = 정션·심볼릭 링크 → 따라가지 않고 "연결 지점"으로 셈, 2a와 같음).
   - 이름 없는 `$DATA`: 상주면 값 길이, 비상주면 **첫 조각(lowest VCN 0)의 data size**. 이름 있는 `$DATA`(대체 스트림, 예: `Zone.Identifier`)는 크기에 넣지 않는다(탐색기 크기와 같음). 폴더 크기는 0(2a와 같음).
   - `$FILE_NAME`: 부모 FRN + 이름 + 네임스페이스. **DOS(8.3) 이름(네임스페이스 2)은 버리고** Win32·Win32&DOS·POSIX 이름을 쓴다.
   - **확장 레코드**(base 참조가 있는 레코드, `$ATTRIBUTE_LIST`로 갈라진 속성): 전체 읽기에서는 그 `$FILE_NAME`·`$DATA`를 base 레코드에 합친다(순서 무관).
4. 트리: 부모 레코드 → 이름들(CSR 배열)을 만든 뒤 루트 폴더(레코드 5)부터 너비 우선으로 넣는다(부모 번호 < 자식 번호 유지).
   부모의 순번(sequence)이 이름의 부모 참조와 다르면(옛 폴더 아래 남은 이름) 넣지 않고 "부모 없는 이름"으로 센다.
   **레코드 16 아래(`$MFT`, `$LogFile`, `$Bitmap`, `$Extend`(와 그 아래 `$UsnJrnl`·`$ObjId`…) 등 메타데이터)는 넣지 않는다** — `C:\`를 FindFirstFile로 훑어도 안 보이는 것들.
   제외 규칙(`$Recycle.Bin`, `System Volume Information`, `?:\Windows\WinSxS`, 사용자 패턴)과 색인 폴더 건너뛰기는 폴더 훑기와 같은 함수(`Excludes::matches`)로.
5. **하드 링크**: 이름(부모, 이름)마다 항목 하나 — FindFirstFile이 링크마다 한 줄을 돌려주는 것과 같다(2a와 항목 수가 맞음). 모든 링크가 같은 크기·시각을 갖는다
   (2a는 디렉터리 항목에 복사된 값이라 링크마다 다를 수 있음 — 아래 "2a와 다른 점").
6. 항목마다 **FRN**(레코드 번호 48비트 + 순번 16비트)을 색인의 새 열에 둔다(저널 변경을 항목에 잇는 데 씀).

`ntfs.rs`는 플랫폼 무관(`VolRead` 트레잇): Windows는 볼륨 핸들, K12 시험은 `mkntfs`로 만든 이미지 파일을 같은 코드로 읽는다.
MFT 읽기가 중간에 실패하면 그 루트는 폴더 훑기로 한다.

### USN 저널

- **색인 형식 v2**(`index-v2.bin`): 루트마다 원본(훑기/MFT)·**볼륨 일련번호·저널 ID·다음 USN**, 항목마다 FRN 열(MFT 루트가 있을 때만). v1(2a) 파일도 읽는다(원본 = 훑기, FRN 없음). v2를 저장하면 v1 파일은 지운다.
- 전체 읽기 **직전에** `FSCTL_QUERY_USN_JOURNAL`로 저널 ID·다음 USN을 적어 둔다 → 읽는 동안 일어난 변경도 그 뒤에 따라잡힌다.
- **시작 때 따라잡기**: 저장한 MFT 루트이고 (일련번호, 저널 ID)가 같고 저장한 USN ≥ 저널의 첫 USN이면, 전체 읽기 대신 `FSCTL_READ_USN_JOURNAL`(V2·V3 레코드)로 그 뒤 기록만 읽는다.
  저널이 다시 만들어졌거나(ID 다름), 한 바퀴 돌아 저장 위치가 지워졌거나(`ERROR_JOURNAL_ENTRY_DELETED`), 저널이 없으면 MFT 전체 다시 읽기.
- **반영 방식**: 기록에서 바뀐 FRN만 모아(중복 제거, 처음 나온 순서) 파일마다 **지금 상태**를 `FSCTL_GET_NTFS_FILE_RECORD`로 다시 읽어 맞춘다
  (속성 목록이 있으면 확장 레코드도 읽음). 그래서 이유(reason) 비트를 해석하지 않고, 같은 변경을 두 번 반영해도 결과가 같다.
  - 레코드가 없거나 순번이 다름 → 지움(아래째). 레코드가 다른 파일로 다시 쓰였으면 옛 항목을 지운다.
  - 있어야 할 (부모, 이름)과 지금 항목을 맞춰: 같으면 크기·시각·대소문자 고침, 파일은 없어진 자리 지우고 새 자리 추가, **폴더는 아래째 옮김**
    (`Index::relocate`: "부모 번호 < 자식 번호"를 지키려고 끝에 복사하고 옛 것은 지움 표시).
  - 부모를 아직 모르면(같은 묶음에서 나중에 만들어진 폴더) 다음 차례에 다시(최대 4번). 끝내 모르면(제외한 폴더 아래 등) 버린다.
  - 다른 곳(제외한 폴더, 예: 휴지통에서 되살림)에서 들어온 폴더는 아래 항목이 저널에 안 나오므로 `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)`로 목록(FRN 포함)을 읽어 채운다.
  - 상태는 잠금 없이 먼저 읽고, 맞추기만 쓰기 잠금 안에서.
- **켜져 있는 동안**: MFT 루트는 `ReadDirectoryChangesW`를 쓰지 않고 저널을 0.5초마다 읽어(더 있으면 바로 이어 읽음) 0.3초 모아 반영. 반영한 뒤 루트의 다음 USN을 갱신(저장 때 함께 저장).
  저널이 끊기면(재생성·한 바퀴) 그 루트를 MFT로 다시 읽는다. 저널이 없는 NTFS 볼륨은 MFT로 읽고 실시간은 `ReadDirectoryChangesW`.
- CLI `filetool-cli usn-catchup [--index-dir]`: 저장한 색인의 MFT 루트마다 따라잡기 → 반영 → 저장(USN 범위, 기록 수, 바뀐 파일 수, 추가/삭제/변경/이동, 시간 출력).

### 2a와 다른 점(같은 볼륨을 두 길로 읽을 때)

- 관리자 아닌 폴더 훑기는 권한 없는 폴더(desktop 1,095개)를 건너뛰지만 MFT는 다 본다. 관리자 토큰이면 둘 다 거의 같다.
- 크기·시각: 2a(FindFirstFile)는 **디렉터리 항목에 복사해 둔 값**(NTFS가 늦게 고침, 하드 링크는 마지막으로 연 링크만 최신)을, MFT는 파일 레코드의 값(정본)을 쓴다. 그래서 열려 있는 로그·DB 파일, 하드 링크(WinSxS↔System32)는 값이 다를 수 있다.
- 스캔 도중 생기고 없어진 파일, 볼륨 마운트 지점 아래(다른 볼륨)는 양쪽 모두 넣지 않는다.

### 권한 상승 방안 (2b 때 정리 — 사용자가 10-31에 **C**를 골라 2c에서 구현, 아래 "2c")

| 방안 | 어떻게 | 좋은 점 | 나쁜 점 |
|---|---|---|---|
| A. 지금 그대로(관리자로 실행했을 때만) | 앱을 "관리자 권한으로 실행"하면 자동으로 MFT | 추가 코드·설치 없음. 보안 경계 그대로 | 대부분 사용자는 2a 길만 씀. 관리자로 앱 전체(웹뷰 포함)를 돌리는 것은 권하지 않음 |
| B. UAC 도우미(필요할 때 한 번) | 작은 `filetool-mftd.exe`를 `runas`로 띄워(UAC 창 1번) MFT를 읽고 결과(색인 파일)를 사용자 폴더에 쓰고 끝남. 실시간은 2a 길 | 설치 불필요, 권한 있는 코드가 작고 짧게 돎 | 시작할 때마다(또는 "빠른 다시 색인" 누를 때마다) UAC 창. 저널을 계속 따라가려면 도우미가 살아 있어야 해서 결국 상주 필요 |
| C. 설치 시 Windows 서비스 | 설치 파일(관리자 설치로 바꿈)이 LocalSystem/LocalService 서비스 등록. 서비스가 MFT·저널을 맡고 앱과 이름 있는 파이프로 통신 | Everything과 같은 체감(UAC 없이 늘 빠름, 실시간 저널) | 지금의 "사용자별 설치(관리자 불필요)"를 포기하거나 따로 설치 단계. 파이프 보안(호출자 확인·경로 노출 범위), 서비스 갱신·제거, 백신 오탐 가능성. 다른 사용자의 파일 이름까지 보이게 되므로 결과를 호출자 권한으로 거를지 결정 필요 |
| D. 예약 작업(가장 높은 권한) | 설치 때 한 번 UAC로 "가장 높은 권한으로 실행" 예약 작업을 만들고, 앱이 그 작업을 시작해 도우미를 띄움 | 그 뒤로는 UAC 없음, 서비스보다 단순 | 사실상 권한 상승 우회 통로라 보안 검토 필요, 작업 만들 때는 관리자 필요, 회사 PC 정책에서 막힐 수 있음 |

추천 순서(의견): 개인 PC 대상이면 C(서비스) 또는 D, 회사·공용 PC가 많으면 A 유지 + B를 선택 기능으로.

### 잰 값 (2b, 2026-10-31)

- K12 `cargo test -p filetool-core`: 67 통과 / 0 실패 / 1 무시(벤치). 새 `search_2b` 10개: 손으로 만든 FILE 레코드(fixup 엄격·너그러움, 상주·비상주 `$DATA`,
  대체 스트림 제외, 속성 목록 + 확장 레코드, 네임스페이스·하드 링크, 지운 레코드·고아·메타데이터·정션), 제외 규칙, USN V2/V3(V4 건너뜀), 색인 v1→v2,
  따라잡기 반영(이동·이름 바꾸기·폴더째 이동·삭제·새 폴더와 그 안·휴지통에서 되살림·하드 링크·레코드 재사용·같은 변경 두 번), 저널 재생성·한 바퀴·다른 볼륨 감지,
  **진짜 NTFS 이미지**(K12에서 sudo 없이 `mkntfs` + `ntfs-3g` FUSE로 64 MB 이미지를 만들어 432개 항목(한글·NFD 이름, 300개 폴더, 3 MB 파일, 하드 링크 121개 → 확장 레코드 25개)을 채우고,
  이미지의 `$MFT` 직접 읽기 결과와 마운트 폴더 훑기 결과를 비교: 경로·종류·크기·수정 시각 모두 같음).
- desktop `C:\`(ssh = 관리자 토큰, 디스크 캐시 따뜻함): MFT 레코드 2,057,472개(쓰는 것 1,962,7xx, 확장 29,463, 깨짐 0), `$MFT` 2,009 MB 읽기 3.1~3.3 초 + 트리 0.7 초 = **스캔 3.8~4.0 초**,
  같은 볼륨 폴더 훑기(관리자) 3.7~3.9 초. 즉 **이 PC(캐시가 따뜻할 때)에서는 MFT가 빠르지 않다** — `$MFT` 2 GB를 읽는 시간이 훑기와 비슷. MFT 길의 이점은
  권한 없는 폴더까지 다 보는 것과 **USN 따라잡기**(다음 시작 때 전체 스캔 없음)·저널 실시간이다. 차가운 캐시(재부팅 직후)는 재지 않음.
- 항목 수: MFT 1,814,365 = 훑기 1,814,365(같은 순간 비교, 경로 차이는 서로의 색인 폴더 2개씩뿐). 크기 다름 108~112개·시각 다름 104~111개 — 모두 열려 있는 로그·DB(-wal, .log, journal)처럼
  디렉터리 항목의 값이 늦게 고쳐지는 파일(MFT 쪽이 정본). 무작위 20개 `Get-Item` 대조: **20/20 같음**(크기·수정 시각).
- 색인 파일 106.3 MB(FRN 열 +13.9 MB; 훑기 92.4 MB), 저장 0.2~1.8 초, CLI 읽기 0.22~0.25 초, 앱·`search-live` 불러오기 0.78 초.
- **따라잡기**: `usn-catchup` 233건 기록 / 바뀐 파일 48개 → 37 ms(+ 저장 0.6 초). 서비스 시작 때 따라잡기 0.04~0.05 초(기록 16~1,189건).
  시험 폴더의 새 파일·이름 바꾸기·삭제·크기 변경·폴더째 이동·새 폴더와 그 안이 모두 반영됨(`desktop-result-usn.txt`).
- **저널 실시간**(앱이 켜진 동안, `ReadDirectoryChangesW` 없이): 만들기 0.65 초, 이름 바꾸기 0.76 초, 새 폴더 0.77 초, 폴더째 이동 0.68 초, 삭제 0.77 초.
- 관리자 아님(예약 작업, Medium): `--source auto` → 일반 색인(1,728,862 항목, 건너뜀 1,169 — 2a와 같음), `--source mft`·`usn-catchup` → 한국어 안내 + 종료 코드 2.
- 화면: 관리자 권한 예약 작업(`/rl highest`, 로그인 세션)으로 앱을 띄우면 상태 줄에 "C:\ 빠른 색인(MFT)", 관리자 아니면 "C:\ 일반 색인".

### 2b에서 찾아 고친 것

- `FSCTL_GET_NTFS_FILE_RECORD`는 **캐시의 레코드를 fixup이 풀린 채로** 준다. 섹터 끝은 진짜 데이터이고 배열 값은 마지막으로 디스크에 쓸 때의 것이라,
  디스크 읽기처럼 엄격히 검사하면 방금 바뀐 레코드(예: 옮긴 폴더)를 "깨짐 → 지워짐"으로 오판했다. 캐시 레코드는 모든 섹터 끝이 시퀀스 번호일 때만 되돌린다(`parse_cached_record`).

## 2c — 빠른 검색 서비스(선택 설치, 사용자 결정 방안 C)

범위: 관리자 권한 없이 실행한 앱도 MFT/USN 빠른 길을 쓰게 한다. **앱 설치는 지금처럼 사용자별·관리자 불필요**로 두고, 서비스는 [설정] > 검색의
"빠른 검색 서비스 설치 / 제거"로 따로(UAC 1번) 넣고 뺀다. 서비스가 없거나 멈추면 앱은 아무 알림 없이 2a/2b 길(관리자면 MFT, 아니면 폴더 훑기)로 돈다.

### 구조

```
win-filetool.exe (사용자, Medium)                       filetool-svc.exe (서비스 FileToolSvc, LocalSystem)
 ├ 앱 색인(SearchService): 서비스가 맡지 않은 루트만      ├ SearchService: NTFS 고정 드라이브 전체(볼륨이 열리는 것), source auto → MFT
 │   (폴더 루트, NTFS가 아닌 드라이브, 서비스 없을 때 전부)  │   색인 %ProgramData%\kr.salarycrew.winfiletool\index\index-v2.bin (2b v2 형식)
 ├ Remote: 1초마다 status(없으면 2초마다 다시 연결)        │   시작: 저장한 색인 불러오기 → 저널 따라잡기(2b try_catch_up 그대로) / 없으면 MFT 전체 읽기
 │   서비스가 다 색인한 루트 = "covered" → 앱 색인에서 뺌    │   켜져 있는 동안 USN 저널 0.5초마다 → 0.3초 모아 반영, 10분마다·멈출 때 저장
 └ 질의: 서비스(앞) + 앱 색인(뒤) 결과를 이어 붙임   ⇄ 파이프 \\.\pipe\kr.salarycrew.winfiletool.search ⇄  └ 파이프 서버(연결마다 스레드, 최대 16) + 가시성 거르기
filetool-cli search --via service / svc-status / svc-watch    (같은 클라이언트)
```

- 앱은 시작할 때 최대 1.5초 서비스를 기다려 본 뒤 앱 색인을 시작한다(서비스가 맡을 루트는 처음부터 빼서 다시 훑지 않게). 그 뒤 감시 스레드가
  서비스가 생기거나(설치·시작) 사라지면(멈춤·제거·파이프 오류) 앱 색인 루트를 바꾼다(`SearchService::reconfigure`). 서비스가 사라지면 그 드라이브를
  앱이 다시 훑는다(desktop C:\ 관리자 아님 3.9초). 질의·줄 요청이 실패해도 앱 색인만으로 답하고 다음 상태 확인 때 루트가 돌아온다.
- 서비스가 아직 처음 색인 중(`ready`가 아니거나 루트가 스캔 중)이면 맡긴 것으로 보지 않는다(앱 색인이 그대로 답함).
- 결과 순서: **서비스 결과 전체(정렬됨) 뒤에 앱 색인 결과(정렬됨)**. 두 원본을 섞어 한 번에 정렬하지 않는다(보통은 서비스가 모든 루트를 맡아 한쪽만 있음).
- 상태 줄: 서비스가 맡은 루트는 "C:\ 빠른 색인(서비스)"(마우스를 올리면 "빠른 검색 서비스(pid …)가 색인… · 다른 사용자의 프로필 폴더 제외"), 항목 수·실시간 반영 수는 둘을 더함.
  버전·세대도 둘을 더한 값이라 어느 쪽이 바뀌어도 화면이 다시 검색한다(색인이 바뀌었는지는 상태 폴링으로 안다 — 알림 푸시는 없음).

### 파이프 프로토콜 (`search/svcproto.rs`, 버전 1)

- 틀: `u32` 길이(LE) + JSON. 봉투 `{"v":1,"op":…}`. 버전이 다르면 `version` 오류, 모르는 op·깨진 JSON은 `bad_request`. 첫 요청은 반드시 `hello`(아니면 `hello_first`).
- 요청: `hello{client}` → `hello{server, protocol, client{user, sid, admin, visibility}}` · `status` → 항목 수·준비·버전/세대·루트별 원본·스캔 통계·따라잡기 결과·시작→준비 시간·클라이언트 수 ·
  `query{q, sort, desc, limit, excludes}` → `{total, millis, generation, version, rows(첫 limit줄)}`(결과 번호는 연결에 둔다) · `rows{offset, limit}` → `{rows, stale}`(세대가 바뀌었으면 stale = 다시 질의).
  **쓰기·지우기·열기·실행 요청은 없다.**
- 한도: 요청 틀 64 KiB(넘으면 본문을 읽지 않고 `limit` 오류 후 끊음), 질의 4,096 바이트, 사용자 제외 패턴 64개·각 520 바이트, 한 번에 줄 1,000개(넘으면 줄임),
  동시 연결 16(파이프 인스턴스 수), 응답 16 MiB(클라이언트 쪽). 클라이언트 요청 하나 10초 시간 제한(겹친 I/O, 넘으면 끊고 앱 색인으로).
- 질의 언어·정렬은 앱과 같은 코드(`query::run`/`sort`). 사용자의 "제외할 폴더" 중 서비스 기본 목록에 없는 것은 결과에서 거른다(폴더와 그 아래, 질의마다 위 폴더를 따라 올라가며 확인 + 메모).

### 파이프 보안

- DACL(SDDL) `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;IU)`: SYSTEM·관리자 전체, **대화형 사용자(IU)는 읽기·쓰기만**(`0x12019b` = FILE_GENERIC_READ|WRITE에서
  `FILE_CREATE_PIPE_INSTANCE`(=APPEND_DATA) 뺌 → 일반 사용자는 같은 이름의 서버 인스턴스를 더 만들 수 없다). 상속 없음(`P`). 그 밖(네트워크·일괄 로그온의 일반 사용자, 익명)은 항목이 없어 거부.
- `PIPE_REJECT_REMOTE_CLIENTS`: 다른 PC(SMB)에서 오는 연결은 받지 않는다. 처음 만든 인스턴스는 `FILE_FLAG_FIRST_PIPE_INSTANCE`(이름을 먼저 차지한 프로세스가 있으면 실패 → 5초마다 다시, 기록에 남김).
- **네트워크 로그온 SID(NU) 거부는 두지 않았다**: 처음엔 `(D;;GA;;;NU)`를 넣었는데 desktop의 ssh 세션(관리자)도 네트워크 로그온이라 막혔다(오류 5). 관리자는 어차피 볼륨을 직접 읽을 수 있고,
  일반 사용자의 ssh 세션 토큰에는 IU가 없어 그대로 막히므로 뺐다.
- 호출자: 서비스는 첫 요청을 읽은 뒤 `ImpersonateNamedPipeClient` → `OpenThreadToken` → `RevertToSelf`로 사용자 SID·그룹을 읽는다(안 되면 `GetNamedPipeClientProcessId` → 프로세스 토큰).
  클라이언트는 `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`으로 열어 서비스가 **확인만** 하고 그 사용자로 행동할 수 없게 한다.
  관리자 = 토큰에 Administrators(S-1-5-32-544)가 **켜져** 있음(권한 상승 안 된 관리자 계정의 Medium 토큰은 그 그룹이 deny-only라 일반 사용자로 본다) 또는 SYSTEM.
- 가짜 서버 막기: 클라이언트는 연결한 뒤 `GetNamedPipeServerProcessId`가 SCM(`QueryServiceStatusEx`)이 아는 FileToolSvc 프로세스 번호와 같은지 확인하고, 다르면 연결하지 않는다.

### 가시성 (다른 사용자의 파일 이름)

서비스는 볼륨의 **모든** 이름을 본다(권한 없는 폴더까지). 그래서 질의마다 호출자에 따라 거른다:

- **관리자(권한 상승된 토큰)**: 모두 보인다(관리자는 볼륨을 직접 읽을 수 있으므로 더 숨길 이유가 없음).
- **그 밖**: 프로필 폴더들의 부모(`ProfileList\ProfilesDirectory`, 보통 `C:\Users`) **바로 아래 폴더는 자기 프로필(`ProfileList\<SID>\ProfileImagePath`)과 `Public`만 보이고 나머지는 숨긴다** —
  다른 사용자, `Default`, 프로필 목록에 없는 폴더(예: 관리자가 만든 `C:\Users\wft2c-other`)도. 프로필 목록의 다른 경로(`C:\Windows\ServiceProfiles\LocalService`,
  `…\system32\config\systemprofile` 등 C:\Users 밖)도 숨긴다. 숨긴 폴더는 이름 자체와 그 아래 전부가 빠지고, 결과 수에도 들어가지 않는다(숨긴 수도 알려 주지 않음).
- 규칙은 질의 때마다 색인에서 다시 찾는다(폴더 이름이 바뀌어도 맞음). 2a 기본 제외 목록(`$Recycle.Bin`, `System Volume Information`, `?:\Windows\WinSxS`)은 서비스가 색인할 때 이미 뺀다.
- **숨기지 않는 것**(알고 둔 것): C:\Users 밖의 권한 없는 폴더 이름(예: `C:\Windows\System32\config\…`, 다른 앱의 `C:\ProgramData\…` 하위) — 2a 폴더 훑기(관리자 아님)는 이런 폴더 1,169개를
  못 들어가지만 서비스 길에서는 이름이 보인다. 내용은 여전히 열 수 없다(파일 열기는 사용자 권한으로).

### 계정·권한

- **LocalSystem**으로 등록하되 서비스 SID(UNRESTRICTED)를 켜고 **필요한 권한만**(`SeChangeNotifyPrivilege`, `SeBackupPrivilege`, `SeManageVolumePrivilege`) 남긴다(`sc qprivs`로 확인).
  실패 시 10초·60초 뒤 다시 시작, 지연된 자동 시작.
- **가상 계정(`NT SERVICE\FileToolSvc`)은 시도했지만 안 된다**(`install --account virtual`로 남겨 둠, 시험용): LSA로 `SeBackupPrivilege`·`SeManageVolumePrivilege`를 주고 시작할 때 켜도(`AdjustTokenPrivileges` 확인),
  `\\.\C:`를 `GENERIC_READ`로 여는 것이 **접근 거부(오류 5)** — 백업 의미(`FILE_FLAG_BACKUP_SEMANTICS`)로 다시 열어도 같음(desktop). 볼륨 장치의 보안 설명자가 이 계정을 허용하지 않는 것으로 보인다(볼륨 DACL 자체는 읽어 보지 않음).
  볼륨 DACL을 바꾸는 것은 시스템 변경이라 하지 않았다. 그래서 기본은 LocalSystem.
- 처음 전체 읽기(저장한 색인이 없을 때)만 프로세스 배경 모드(`PROCESS_MODE_BACKGROUND_BEGIN`: 아주 낮은 I/O·CPU 우선순위)로 돌리고 준비되면 끈다.
  저장한 색인을 불러오는 다시 시작은 보통 우선순위(배경 모드로 불러오면 111 MB에 13.2초 → 끄니 0.8초).

### 설치 / 제거 (`filetool-svc install|uninstall|status`)

- **설치(관리자)**: ① 이미 있으면 멈춤 ② `%ProgramFiles%\파일 도구 서비스\`를 만들고 DACL `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)`(Users 읽기·실행만)로 바꾼 뒤
  자신을 복사(`.new`에 쓰고 바꿔치기) ③ 그 경로를 `"…\filetool-svc.exe" run`으로 등록(있으면 설정 바꿈) + 설명·지연 시작·SID·권한·실패 동작
  ④ `%ProgramData%\kr.salarycrew.winfiletool\`(+ `index\`)를 SYSTEM·관리자만(`D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`)으로. 이미 있는데 소유자가 SYSTEM·관리자가 아니면
  (일반 사용자가 먼저 만들어 둔 폴더 — 소유자는 DACL을 다시 바꿀 수 있으므로) 지우고 새로 만든다 ⑤ 시작 → 실행 중 확인 → 파이프로 상태를 받아 출력.
  다시 설치(갱신)는 색인을 지우지 않는다. `--report <파일>`이면 출력을 파일에도 쓴다(앱이 창 없이 UAC로 띄운 실행의 결과를 읽으려고).
- **왜 Program Files로 복사하나**: 앱 폴더(`%LOCALAPPDATA%\파일 도구\`)는 사용자가 쓸 수 있다. 거기 있는 exe를 서비스(LocalSystem)로 등록하면 사용자가 exe를 바꿔치기해
  SYSTEM 권한을 얻는다(권한 상승). 그래서 관리자만 쓸 수 있는 곳에 복사해 그 경로를 등록한다. desktop에서 일반 사용자가 그 폴더에 쓰기·exe 바꿔치기 → 거부 확인.
- **제거(관리자)**: 멈춤(최대 60초) → `DeleteService` → (가상 계정이면 LSA 권한 거둠) → 등록이 사라질 때까지 기다림 → ProgramData 폴더 → Program Files 폴더 지움.
  Program Files 안의 자신으로 제거하면 실행 중인 exe는 지울 수 없어 다시 시작할 때 지우도록 예약(`MoveFileEx` DELAY_UNTIL_REBOOT)한다. 앱의 단추·설치 파일은 앱 폴더의 사본으로 지우므로 바로 다 지워진다.
- `status`(관리자 아니어도 됨): SCM 상태·시작 유형·계정·실행 파일, 서비스 폴더·자료 폴더·파이프의 SDDL, 파이프로 받은 색인 상태.
- **앱 [설정] > 검색 > 빠른 검색 서비스**: 설치 상태 글 + "설치(다시 설치)" / "제거" 단추. 단추는 앱 옆 `filetool-svc.exe`를 `ShellExecuteExW("runas", "install|uninstall --report %TEMP%\win-filetool\svc-setup.txt")`로
  실행하고(UAC 창 1번, 창은 숨김) 끝날 때까지 기다린 뒤 보고서 글을 결과 칸에 보여 준다. UAC에서 [아니요]면 "관리자 권한 확인(UAC)을 취소했습니다". 실행할 명령 줄은 단추 아래에 그대로 보인다.
- **앱 제거(NSIS)**: 제거 프로그램이 `HKLM\SYSTEM\CurrentControlSet\Services\FileToolSvc`를 보고 서비스가 있으면 "함께 제거할까요?(관리자 권한 확인 창)"를 묻는다.
  [예] → 앱 폴더의 `filetool-svc.exe uninstall`을 runas로. [아니요]·조용히 제거(`/S`)는 서비스를 남긴다(서비스 exe는 Program Files에 따로 있어 계속 돈다 —
  나중에 관리자 명령 프롬프트에서 `"C:\Program Files\파일 도구 서비스\filetool-svc.exe" uninstall`). desktop에서는 `/S` 경로만 확인(서비스가 남음), 묻는 창은 누를 수 없어 확인 못 함.

### 위협 메모

| 위협 | 막는 것 | 남는 것 |
|---|---|---|
| 일반 사용자가 서비스 exe를 바꿔 SYSTEM으로 실행 | exe는 Program Files(관리자만 쓰기)에서 실행, 경로를 따옴표로 등록 | 설치 단계 자체는 앱 폴더(사용자 쓰기 가능)의 exe를 UAC로 실행한다 — 같은 사용자의 악성 코드가 그 exe를 미리 바꿔 두면 사용자가 UAC에 [예]를 누를 때 관리자 권한을 얻는다(사용자별 설치 프로그램이 UAC를 쓸 때의 일반적 한계, UAC 창에 서명 없는 게시자로 보임) |
| 색인 파일을 직접 읽어 가시성 우회 | ProgramData 폴더 SYSTEM·관리자만, 먼저 만들어 둔 폴더는 지우고 새로 | — |
| 가짜 파이프 서버(이름 선점)로 앱에 거짓 결과 | 서버 pid = SCM 서비스 pid 확인, 서비스 쪽 첫 인스턴스 플래그, IU에 인스턴스 만들기 권한 없음 | 서비스가 멈춘 사이 선점하면 앱은 "서비스 아님"으로 연결하지 않고 앱 색인으로 동작 |
| 다른 PC에서 파이프로 목록 빼내기 | `PIPE_REJECT_REMOTE_CLIENTS` + DACL에 네트워크 사용자 항목 없음 | 관리자 ssh(로컬 네트워크 로그온)는 들어옴(관리자라 의미 없음) |
| 다른 사용자 파일 이름 노출 | 가시성 규칙(다른 프로필 폴더 숨김) | C:\Users 밖 권한 없는 폴더의 이름은 보임(위) |
| 서비스로 파일을 열거나 지우기 | 프로토콜에 그런 요청이 없음(질의·상태만) | — |
| 요청으로 서비스 마비(DoS) | 요청 64 KiB·질의 4 KB·줄 1,000·연결 16, 쓰기 요청 없음 | 대화형 사용자가 연결 16개를 붙잡거나 무거운 질의를 반복하면 다른 사용자의 질의가 늦어질 수 있다(같은 PC 사용자 사이) |
| 백신 오탐 | — | 서명하지 않은 서비스 exe(코드 서명은 범위 밖) |

### 잰 값 (2c, 2026-10-31, desktop C:\ 181만 항목, 캐시 따뜻함)

- 설치 1.78 초(파이프 응답까지). **처음 색인(배경 모드)**: MFT 스캔 9.3~15.6 초, 이름 순위·저장(111 MB)까지 **준비 28.4~33.7 초**(같은 때 관리자 CLI `--source mft` 보통 우선순위 3.7~5.6 초).
  항목 1,814,549(서비스) vs 1,814,553(CLI MFT, 같은 순간이 아님 — 차이는 서로의 색인 폴더 2개와 그 사이 바뀐 로그 파일).
- **다시 시작**: 멈춤 0.53 초, 시작 → 준비 **1.00 초**(불러오기 0.80 초 + 저널 따라잡기 0.04 초, 멈춘 동안 만든 파일 반영).
- 질의(일반 사용자, 파이프 왕복): `notepad.exe` 중간 3.7 ms(3.3~4.0), `보고서` 3.9 ms, `ext:jpg;png;heic`(16만 개) 15.6 ms, 경로 낱말 8.3 ms, 연결 1.2~1.4 ms.
- 실시간(서비스 저널 → 일반 사용자 질의, 50 ms 폴링): 만들기 0.67 초, 이름 바꾸기 0.68 초, 지우기 0.64 초.
- 앱(Medium): 창 뒤 1.2 초에 "C:\ 빠른 색인(서비스)", 서비스 멈춤 → 0.28 초 뒤 "C:\ 일반 색인"(앱이 C:\ 훑기 3.9 초), 다시 시작 → 1.5 초 뒤 다시 서비스. 오류 표시 없음.

## ux3 — 작은 고침 + 쓰기 편하게

### 감시 잠금 경쟁(1b `watch_lock_single_holder`가 가끔 실패)

- 원인: Linux에서 std `File::try_lock`은 `flock`이고, `flock` 잠금은 **열린 파일 설명**에 붙는다. 같은 시험 바이너리의 다른 시험 스레드가
  자식 프로세스를 띄우면(fork ~ exec 사이) 자식이 fd 표를 복사해 가므로, 부모가 잠금 파일을 닫아도 자식이 exec할 때까지 잠금이 남는다.
  그 틈에 "놓은 뒤 다시 잡기"가 실패했다. 재현: 스레드 4개가 `true`를 계속 띄우는 동안 잡기→놓기→다시 잡기 2000번 → 고치기 전 실패.
  K12에서 고치기 전 `convert_1b` 30번 중 2번 실패(같은 메시지 "풀린 뒤 다시 잡음").
- 고침(잠금 자체): Unix는 프로세스 단위 `fcntl(F_SETLK)` 잠금(fork로 물려지지 않음) + 같은 프로세스 안의 두 번째 요청을 막는 "잡은 경로" 목록
  (POSIX 잠금은 같은 프로세스끼리 충돌하지 않고, 같은 파일의 다른 fd를 닫으면 풀리므로 목록을 잡은 채로 열고/닫는다).
  Windows는 그대로 `LockFileEx`(핸들이 상속되지 않아 이 문제가 없음). 시험: `tests/lock_race_ux3.rs`(동시 spawn 중 2000번, 다른 프로세스 배제).

### [폴더 열기]

- 결과 줄(`log(…, target)`)에 단추. `{ select: 파일 }` → `open_folder`(`explorer /select,"…"`), `{ dir: 폴더 }` → 새 명령 `open_dir`(폴더 자체).
  여러 파일 작업: 나누기(파일이 둘 이상이면 첫 결과의 폴더), 쪽마다 그림(한 장이면 그 파일), 사진 꺼내기, zip 풀기, NFC 이름 고치기. 감시 기록은 결과 파일 선택.

### 화면 상태 기억 (`settings.json` → `ui`)

- `tab`, `window { x, y, width, height, maximized }`(물리 픽셀, 바깥 위치 + 안쪽 크기), `sort_key`/`sort_desc`, `col_widths`(이름·크기·날짜 px, 폴더 열은 남는 만큼).
- 창: `Moved`/`Resized` 이벤트에서 메모리만 바꾸고 0.8초 조용하면 저장(끄기 직전에도 저장). 숨김·최소화 중에는 기록하지 않고, 최대화는 표시만 바꿔 보통 크기를 지킨다.
- 시작: 창이 숨은 채로 기억한 값을 **지금 모니터들의 작업 영역 안으로** 맞춘 뒤(`WindowGeom::clamp_to`: 가장 많이 겹치는 모니터 안으로 밀어 넣고,
  겹치는 모니터가 없으면 주 모니터 가운데, 모니터보다 크면 줄임) 보인다. 탭 우선순위: `--tab=` 인자 > 기억한 탭 > 검색.

### 파일 아이콘

- `file_icons(keys)`: 키 = 소문자 확장자 / `""`(확장자 없음) / `"/dir"`. Windows `SHGetFileInfoW(SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES)`
  — 파일을 열지 않고 확장자·속성만으로 받는다(.exe·.lnk·.ico처럼 파일마다 다른 아이콘은 같은 종류의 기본 아이콘). HICON → 32비트 BGRA(`GetDIBits`,
  알파 없는 옛 아이콘은 마스크로) → PNG → `data:` URL. 앱 쪽(Rust)과 화면 쪽(JS) 둘 다 키마다 한 번만. 화면은 그리는 줄(보이는 줄 ± 5)의 키만 모아 한 번에 묻는다.
- Windows가 아니면 아이콘 없음 → 글자 아이콘(📄/📁) 그대로.

### `< >` 묶기

- 낱말 나누기에서 `<`(낱말 처음, `!` 뒤 포함)는 `Open{not}`, 묶음 안의 `>`는 `Close`. `size:>1mb`·`dm:<2024`처럼 낱말 가운데 또는 `:` 바로 뒤의
  `<` `>`는 비교 기호로 낱말에 남는다. 해석은 재귀(`parse_seq`): 묶음 하나가 낱말 하나처럼 AND/OR에 들어간다. 닫지 않은 묶음은 끝에서 닫고, 묶음 밖의 `>`는 글자.
