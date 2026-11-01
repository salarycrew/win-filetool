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
- 0.3.0 W2: `quick-xml` 0.42(MIT, Word·PowerPoint·Excel XML 읽기 — 이미 Cargo.lock에 있던 버전)
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
(Windows 시험 PC에서 확인: `msiexec /a LibreOffice_26.2.6_Win_x86-64.msi /qn TARGETDIR=<폴더>` → `<폴더>\program\soffice.exe`, 권한 상승 없이 성공).

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

## 빌드 경로: Linux 빌드 PC에서 크로스 컴파일 → Windows 시험 PC에서 실행

Windows 시험 PC에는 Rust·MSVC Build Tools가 없고 원격 세션은 관리자 권한이 아니다.
그래서 **Linux 빌드 PC에서 `cargo-xwin`으로 `x86_64-pc-windows-msvc` exe를 만들고**, Windows 시험 PC에는 복사만 한다.

Linux 빌드 PC 준비(한 번, 모두 사용자 영역, sudo 없음):

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

Windows 시험 PC 배치·점검:

```sh
scp -r dist-win/* <windows-pc>:C:/Users/user/dev/win-filetool-builds/1c/        # 설치 파일 + 휴대용 묶음(portable\)
ssh <windows-pc> "C:\Users\user\dev\win-filetool-builds\1c\portable\filetool-cli.exe selftest C:\Users\user\dev\win-filetool-builds\1c\selftest --soffice C:\Users\user\dev\win-filetool-builds\tools\lo\program --apple C:\Users\user\dev\win-filetool-builds\1b\apple"
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
- `makensis`는 Linux 빌드 PC에 sudo 없이: `scripts/fetch-nsis.sh`가 Ubuntu `nsis`/`nsis-common` 패키지를 `apt-get download`로 받아 `~/.local/opt/nsis-3.10`에 풀고
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
Windows 시험 PC은 원격에서 권한 상승(UAC)을 할 수 없고, 많은 사용자도 관리자로 실행하지 않는다. 그래서 2a는 **검색 기능 전체**(색인·저장·실시간 갱신·질의·UI)를
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

- Linux 빌드 PC 합성 100만 항목(평균 깊이 8.7): 만들기 312 ms, 색인 배열 110 MB(RSS 증가 73 MB), 파일 40.7 MB, 저장 85 ms, 읽기 235 ms, 이름 순위 540 ms,
  질의 12종 찾기 p95 3.6~28.3 ms, 이름 정렬 포함 p95 6.8~64.3 ms(77만 개 결과). `crates/core/tests/search_bench.rs`
- Windows 시험 PC `C:\`(관리자 아님, Medium): 1,728,731 항목, 스캔 3.66 초(바로 앞 관리자 ssh 스캔 7.07 초 뒤라 캐시가 따뜻함), 건너뜀 1,169(권한 없음 1,095 + 연결 지점 74),
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

`ntfs.rs`는 플랫폼 무관(`VolRead` 트레잇): Windows는 볼륨 핸들, Linux 빌드 PC 시험은 `mkntfs`로 만든 이미지 파일을 같은 코드로 읽는다.
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

- 관리자 아닌 폴더 훑기는 권한 없는 폴더(Windows 시험 PC 1,095개)를 건너뛰지만 MFT는 다 본다. 관리자 토큰이면 둘 다 거의 같다.
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

- Linux 빌드 PC `cargo test -p filetool-core`: 67 통과 / 0 실패 / 1 무시(벤치). 새 `search_2b` 10개: 손으로 만든 FILE 레코드(fixup 엄격·너그러움, 상주·비상주 `$DATA`,
  대체 스트림 제외, 속성 목록 + 확장 레코드, 네임스페이스·하드 링크, 지운 레코드·고아·메타데이터·정션), 제외 규칙, USN V2/V3(V4 건너뜀), 색인 v1→v2,
  따라잡기 반영(이동·이름 바꾸기·폴더째 이동·삭제·새 폴더와 그 안·휴지통에서 되살림·하드 링크·레코드 재사용·같은 변경 두 번), 저널 재생성·한 바퀴·다른 볼륨 감지,
  **진짜 NTFS 이미지**(Linux 빌드 PC에서 sudo 없이 `mkntfs` + `ntfs-3g` FUSE로 64 MB 이미지를 만들어 432개 항목(한글·NFD 이름, 300개 폴더, 3 MB 파일, 하드 링크 121개 → 확장 레코드 25개)을 채우고,
  이미지의 `$MFT` 직접 읽기 결과와 마운트 폴더 훑기 결과를 비교: 경로·종류·크기·수정 시각 모두 같음).
- Windows 시험 PC `C:\`(ssh = 관리자 토큰, 디스크 캐시 따뜻함): MFT 레코드 2,057,472개(쓰는 것 1,962,7xx, 확장 29,463, 깨짐 0), `$MFT` 2,009 MB 읽기 3.1~3.3 초 + 트리 0.7 초 = **스캔 3.8~4.0 초**,
  같은 볼륨 폴더 훑기(관리자) 3.7~3.9 초. 즉 **이 PC(캐시가 따뜻할 때)에서는 MFT가 빠르지 않다** — `$MFT` 2 GB를 읽는 시간이 훑기와 비슷. MFT 길의 이점은
  권한 없는 폴더까지 다 보는 것과 **USN 따라잡기**(다음 시작 때 전체 스캔 없음)·저널 실시간이다. 차가운 캐시(재부팅 직후)는 재지 않음.
- 항목 수: MFT 1,814,365 = 훑기 1,814,365(같은 순간 비교, 경로 차이는 서로의 색인 폴더 2개씩뿐). 크기 다름 108~112개·시각 다름 104~111개 — 모두 열려 있는 로그·DB(-wal, .log, journal)처럼
  디렉터리 항목의 값이 늦게 고쳐지는 파일(MFT 쪽이 정본). 무작위 20개 `Get-Item` 대조: **20/20 같음**(크기·수정 시각).
- 색인 파일 106.3 MB(FRN 열 +13.9 MB; 훑기 92.4 MB), 저장 0.2~1.8 초, CLI 읽기 0.22~0.25 초, 앱·`search-live` 불러오기 0.78 초.
- **따라잡기**: `usn-catchup` 233건 기록 / 바뀐 파일 48개 → 37 ms(+ 저장 0.6 초). 서비스 시작 때 따라잡기 0.04~0.05 초(기록 16~1,189건).
  시험 폴더의 새 파일·이름 바꾸기·삭제·크기 변경·폴더째 이동·새 폴더와 그 안이 모두 반영됨(`result-usn.txt`).
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
  앱이 다시 훑는다(Windows 시험 PC C:\ 관리자 아님 3.9초). 질의·줄 요청이 실패해도 앱 색인만으로 답하고 다음 상태 확인 때 루트가 돌아온다.
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
- **네트워크 로그온 SID(NU) 거부는 두지 않았다**: 처음엔 `(D;;GA;;;NU)`를 넣었는데 Windows 시험 PC의 ssh 세션(관리자)도 네트워크 로그온이라 막혔다(오류 5). 관리자는 어차피 볼륨을 직접 읽을 수 있고,
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
  `\\.\C:`를 `GENERIC_READ`로 여는 것이 **접근 거부(오류 5)** — 백업 의미(`FILE_FLAG_BACKUP_SEMANTICS`)로 다시 열어도 같음(Windows 시험 PC). 볼륨 장치의 보안 설명자가 이 계정을 허용하지 않는 것으로 보인다(볼륨 DACL 자체는 읽어 보지 않음).
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
  SYSTEM 권한을 얻는다(권한 상승). 그래서 관리자만 쓸 수 있는 곳에 복사해 그 경로를 등록한다. Windows 시험 PC에서 일반 사용자가 그 폴더에 쓰기·exe 바꿔치기 → 거부 확인.
- **제거(관리자)**: 멈춤(최대 60초) → `DeleteService` → (가상 계정이면 LSA 권한 거둠) → 등록이 사라질 때까지 기다림 → ProgramData 폴더 → Program Files 폴더 지움.
  Program Files 안의 자신으로 제거하면 실행 중인 exe는 지울 수 없어 다시 시작할 때 지우도록 예약(`MoveFileEx` DELAY_UNTIL_REBOOT)한다. 앱의 단추·설치 파일은 앱 폴더의 사본으로 지우므로 바로 다 지워진다.
- `status`(관리자 아니어도 됨): SCM 상태·시작 유형·계정·실행 파일, 서비스 폴더·자료 폴더·파이프의 SDDL, 파이프로 받은 색인 상태.
- **앱 [설정] > 검색 > 빠른 검색 서비스**: 설치 상태 글 + "설치(다시 설치)" / "제거" 단추. 단추는 앱 옆 `filetool-svc.exe`를 `ShellExecuteExW("runas", "install|uninstall --report %TEMP%\win-filetool\svc-setup.txt")`로
  실행하고(UAC 창 1번, 창은 숨김) 끝날 때까지 기다린 뒤 보고서 글을 결과 칸에 보여 준다. UAC에서 [아니요]면 "관리자 권한 확인(UAC)을 취소했습니다". 실행할 명령 줄은 단추 아래에 그대로 보인다.
- **앱 제거(NSIS)**: 제거 프로그램이 `HKLM\SYSTEM\CurrentControlSet\Services\FileToolSvc`를 보고 서비스가 있으면 "함께 제거할까요?(관리자 권한 확인 창)"를 묻는다.
  [예] → 앱 폴더의 `filetool-svc.exe uninstall`을 runas로. [아니요]·조용히 제거(`/S`)는 서비스를 남긴다(서비스 exe는 Program Files에 따로 있어 계속 돈다 —
  나중에 관리자 명령 프롬프트에서 `"C:\Program Files\파일 도구 서비스\filetool-svc.exe" uninstall`). Windows 시험 PC에서는 `/S` 경로만 확인(서비스가 남음), 묻는 창은 누를 수 없어 확인 못 함.

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

### 잰 값 (2c, 2026-10-31, Windows 시험 PC C:\ 181만 항목, 캐시 따뜻함)

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
  Linux 빌드 PC에서 고치기 전 `convert_1b` 30번 중 2번 실패(같은 메시지 "풀린 뒤 다시 잡음").
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

## 0.3.0 바탕(W1) — 설치 없는 exe 하나 + 앱에 든 도구

### 포장
- `dist-win/파일도구_<버전>_portable.exe` = `win-filetool.exe` + 도구 묶음 `tools.pack`(exe 뒤에 덧붙임, Windows는 무시하고 실행).
  설치본(`_x64-setup.exe`)은 같은 `tools.pack`을 exe 옆에 둔다. 형식·푸는 규칙은 `crates/core/src/bundle.rs` 머리말.
- 묶음 안 도구: `ffmpeg`(ffmpeg.exe·ffprobe.exe, LGPL 최소 빌드 `scripts/build-ffmpeg.sh`), `pdfium`(pdfium.dll + licenses/),
  `lo`(줄인 LibreOffice `scripts/trim-libreoffice.ps1`, poppler 쓰는 xpdfimport 없음), `notices`(THIRD-PARTY-NOTICES.txt).
  도구마다 `tar` → `zstd -19 --long=27` 덩어리라 도구별로 따로 푼다.
- 푸는 곳 `%LOCALAPPDATA%\kr.salarycrew.winfiletool\tools\<앱버전>-<해시16>\<도구>\` (`.partial` → 이름 바꾸기, `.<도구>.lock` 잠금).
  쓸 수 없으면 exe 옆 `tools\`. 새 버전이 풀리면 지난 버전 폴더는 지운다(쓰는 중이면 다음에). 시험용 `WIN_FILETOOL_TOOLS_DIR`, `WIN_FILETOOL_BUNDLE`.
- 도구 찾는 순서: 앱에 든 도구 → 직접 지정한 경로 → PATH → 설치 폴더. `settings.json`의 `tools.prefer_custom: true`(고급)이면 직접 지정한 경로가 먼저.
  ffmpeg는 처음 쓸 때 바로 풀고(0.1초 안), LibreOffice는 첫 문서 작업 때 푼다(약 3초).
- LibreOffice 프로필: `tools\<버전>\lo-profile-1..3` 돌려 쓰기 + 잠금(동시 변환 3개까지, 넘으면 빌 때까지 기다림). 재사용한 프로필로 실패하면 지우고 한 번 더.
- 영상: 앱에 든 ffmpeg는 libx264가 없어 `h264_mf`(Media Foundation). 메신저용은 목표 크기를 넘으면 비트레이트를 낮춰 최대 3번 만든다
  (`media::next_video_kbps`). 사용자가 지정한 ffmpeg에 libx264가 있으면 예전 인자 그대로.
- 휴대용 설정: exe 옆 `win-filetool-data\`(또는 `portable.txt`)가 있고 쓸 수 있으면 `settings.json`·`watch-state.json`을 그 안에.
  검색 색인·풀어 놓은 도구는 언제나 PC의 LOCALAPPDATA.
- WebView2: 창을 만들기 전에 레지스트리(HKLM WOW6432Node / HKLM / HKCU `EdgeUpdate\Clients\{F3017226-…}` `pv`)를 보고, 없으면 한국어 안내 창
  → [확인]이면 Evergreen 부트스트래퍼 주소를 열고 끝낸다.

### 화면과의 약속(Tauri 명령·이벤트) — 파일 `src-tauri/src/cmd_runtime.rs`

| 이름 | 인자 | 돌려주는 값 |
|---|---|---|
| `app_info` | 없음 | `{ name, version, notices, notices_path, config_dir, config_source: "env"\|"portable"\|"default", webview2 }` — [앱 정보] 화면. `notices`는 고지 전문(약 1.3 MB 글) |
| `tools_bundle_status` | 없음 | `{ present, file, dir, tools: [{ name, ready, packed_bytes, bytes }], unpacked: [{ tool, dir, secs, files, bytes }] }` |
| `tools_prepare` | `{ tool: "lo" \| "ffmpeg" \| "pdfium" \| "notices" }` | 푼 폴더 경로(문자열). 묶음이 없으면 오류 글 |

| 이벤트 | 내용 |
|---|---|
| `tools_unpack_progress` | `{ tool, fraction }` (0.0 → 1.0, 2%마다). 문서 변환(`convert_one`)이 처음 LibreOffice를 쓸 때도 자동으로 온다 → "문서 도구 준비 중(약 3초)" 표시, `fraction == 1`이면 닫기 |

- `convert_one` 결과의 `note`에 메신저용 재시도가 있으면 `", 크기를 맞추려고 N번 만듦"`, 끝내 넘으면 `" (목표 X MB보다 큼)"`이 붙는다.
- 오류 글(코어 `Error`의 한국어): 도구 준비 실패 → "앱에 든 도구를 준비하지 못했습니다. 디스크 공간(약 0.5GB)과 폴더 권한을 확인해 주세요. (…)",
  문서 결과 없음 → "결과 파일이 생기지 않았습니다(이 형식을 읽지 못했거나 파일이 손상됐을 수 있음).", 동시 변환이 오래 막힘 → "다른 문서 변환이 끝나지 않아 기다리다 멈췄습니다. 잠시 뒤 다시 해 주세요."
- CLI 점검: `filetool-cli bundle-status | bundle-unpack [도구…] | app-info [--notices] | webview2`.
## 0.3.0 PDF 편집 (W3 코어 — 화면은 W5)

코어 `crates/core/src/pdfedit/`, Tauri 명령 `src-tauri/src/cmd_pdfedit.rs`, CLI `filetool-cli help-pdfedit`(점검: `filetool-cli pdfedit-selftest <빈 폴더>`).
원래 PDF의 글자를 고치는 것은 범위 밖이고, 위에 새 내용을 얹는 것(쪽 번호·워터마크·서명·글상자)은 범위 안이다.

### 공통 규칙

- JS에서 부를 때 인자 이름은 camelCase(`outDir`, `otherPassword`), 반환 JSON 필드는 아래 표의 이름 그대로(snake_case).
- **좌표**: 보이는 쪽 기준 pt(1/72 in), **원점 왼쪽 위**, `/Rotate`를 반영한 모양(= `pdf_thumbs` 그림과 같은 방향). 썸네일 px → pt는 `pt = px × visual_width / 썸네일 너비`.
- **결과**: 원본 옆(또는 `outDir`)에 새 파일 `이름_접미사.pdf`, 같은 이름이 있으면 ` (1)`. 원본은 그대로. 대부분 `EditOut { output, pages, note }`(`note`는 사용자에게 그대로 보여 줄 짧은 한국어).
- **오류**: `{ code, message }`. `code` = `password_required`(암호 PDF — 암호 칸을 띄우고 같은 명령을 `password`를 넣어 다시) · `wrong_password`(암호가 틀림) · `cancelled` · `error`(그 밖, `message`를 그대로 보여 줌). 암호로 연 PDF의 결과는 암호 없이 저장된다(다시 걸려면 `pdf_protect`).
- **진행·취소**: `job: u64`를 받는 명령은 `"job-progress"` 이벤트 `{ job, fraction }`(0~1)을 보내고, 기존 `cancel_jobs`로 멈춘다(만들던 파일은 남지 않음).
- **쪽 선택 문자열**: `"1-3,5"`, `"7-"`, 빈 값/`"all"` = 전체.
- **한글 글꼴**: 앱에 넣지 않는다. 실행 중에 `C:\Windows\Fonts\malgun.ttf`(맑은 고딕)을 찾아 **쓴 글자만** 부분 글꼴로 PDF에 넣는다(넣기 금지 글꼴은 건너뜀, 맑은 고딕은 넣기 허용). 순서: 환경 변수 `WIN_FILETOOL_FONT` → 맑은 고딕 → 나눔고딕 → 굴림. 못 찾으면 `error`("한글 글꼴을 찾지 못했습니다 …").

### 명령

| 명령 | 인자 | 반환 | 접미사 |
|---|---|---|---|
| `pdf_pages` | `path, password?` | `{ pages: [{ page, width, height, rotate, visual_width, visual_height }], encrypted }` (pt, 1부터) | — |
| `pdf_thumbs` | `path, password?, pages: number[], width` (px, 16~2000) | `[{ page, width, height, data_url, ms }]` (`data:image/png;base64,…`) | — |
| `pdf_apply_plan` | `job, plan: EditPlan, outDir?` | `EditOut` | `_쪽편집`(첫 원본 이름) |
| `pdf_extract` | `job, path, password?, pages: "1-3,5", outDir?` | `EditOut` | `_쪽뽑기` |
| `pdf_insert_blank` | `job, path, password?, after (0=맨 앞), count, size: BlankSize, outDir?` | `EditOut` | `_빈쪽추가` |
| `pdf_insert_pdf` | `job, path, password?, after, other, otherPassword?, otherPages?, outDir?` | `EditOut` | `_끼워넣기` |
| `pdf_page_numbers` | `job, path, password?, opts: PageNumberOpts, outDir?` | `EditOut` | `_쪽번호` |
| `pdf_watermark` | `job, path, password?, opts: WatermarkOpts, outDir?` | `EditOut` | `_워터마크` |
| `pdf_place_image` | `job, path, password?, opts: SignatureOpts, outDir?` | `EditOut` | `_서명` |
| `pdf_text_boxes` | `job, path, password?, boxes: TextBox[], outDir?` | `EditOut` | `_글상자` |
| `pdf_crop` | `job, path, password?, opts: CropOpts, outDir?` | `EditOut` | `_여백자름` |
| `pdf_fit_a4` | `job, path, password?, opts: FitOpts, outDir?` | `EditOut` | `_A4` |
| `pdf_nup` | `job, path, password?, opts: NupOpts, outDir?` | `EditOut` | `_모아찍기` |
| `pdf_protect` | `path, password?, opts: ProtectOpts, outDir?` | `EditOut` | `_암호` |
| `pdf_unprotect` | `path, password, outDir?` | `EditOut` | `_암호풀림` |
| `pdf_protection` | `path, password?` | `{ needs_password, encrypted, allow_print?, allow_copy?, allow_edit?, method? }` | — |
| `pdf_info_get` | `path, password?` | `{ title?, author?, subject?, keywords?, creator?, producer?, created?, modified?, xmp_bytes?, pages }` | — |
| `pdf_info_set` | `path, password?, info: DocInfo, outDir?` | `EditOut` | `_정보수정` |
| `pdf_info_clear` | `path, password?, outDir?` | `EditOut` | `_정보삭제` |
| `pdf_form_list` | `path, password?` | `[{ name, kind, value, options, read_only, page? }]` | — |
| `pdf_form_fill` | `job, path, password?, values: {이름: 값}, flatten: bool, outDir?` | `EditOut` | `_양식채움` |
| `pdf_flatten` | `job, path, password?, outDir?` | `EditOut` | `_평평하게` |

### 옵션 JSON (생략한 항목은 기본값)

```jsonc
// EditPlan — 쪽 편집 격자의 결과. 목록 순서 = 새 PDF 쪽 순서. 빼면 지워지고, 같은 쪽을 두 번 넣으면 복제.
{ "sources": [{ "path": "a.pdf", "password": null }, { "path": "b.pdf" }],
  "pages": [ { "kind": "page", "src": 0, "index": 2, "rotate": 90 },   // index 0부터, rotate = 지금 회전에 더할 90의 배수
             { "kind": "blank", "size": "a4" },                        // BlankSize: "same"(앞쪽 크기, 기본)|"a4"|"a4-landscape"|"letter"|"letter-landscape"|{ "w": pt, "h": pt }
             { "kind": "page", "src": 1, "index": 0 } ] }
// 원본이 하나면 양식(AcroForm)·언어·보기 설정을 유지, 책갈피는 지움. 문서 정보는 첫 원본 것.

// PageNumberOpts
{ "position": "bottom-center",   // top-left|top-center|top-right|bottom-left|bottom-center|bottom-right
  "start": 1, "format": "plain",  // plain "1" | of-total "1 / N" | dash "- 1 -"  (N = 마지막 번호)
  "font_size": 10, "margin_x": 28.35, "margin_y": 28.35,  // pt (10 mm)
  "skip_first": false,            // true면 첫 쪽은 비우고 둘째 쪽이 start
  "color": "#000000", "pages": null }

// WatermarkOpts — kind별 필드가 같은 객체에 들어감
{ "kind": "text", "text": "대외비", "font_size": 48, "color": "#808080",
  "opacity": 0.3, "angle": 45,    // 각도: 시계 반대 방향(도)
  "tiling": "center",             // center | tile(바둑판)
  "layer": "over",                // over(내용 위) | under(내용 아래 — 흰 바탕 그림이 있는 쪽에서는 가려질 수 있음)
  "pages": null }
{ "kind": "image", "path": "logo.png", "scale": 0.5 /* 쪽 너비 비율 */, "opacity": 0.3, "angle": 0, "tiling": "center", "layer": "over" }

// SignatureOpts — 서명·도장 그림(PNG/JPG). 한 번에 여러 곳
{ "image": "sign.png", "remove_white": true /* 흰 배경을 투명하게 */, "opacity": 1,
  "placements": [{ "page": 1, "x": 400, "y": 700, "w": 120, "h": 0 /* 0 = 그림 비율 */ }] }

// TextBox[] — 새 글자 얹기(한글 가능)
[{ "page": 1, "x": 80, "y": 160, "text": "확인했습니다\n2026-11-01", "font_size": 12, "color": "#000000",
   "background": "#FFFFFF" /* 없으면 null */, "width": 0 /* >0이면 그 너비에서 줄바꿈 */ }]

// CropOpts
{ "mode": "margins", "top": 20, "right": 20, "bottom": 20, "left": 20, "pages": null }   // 보이는 쪽 기준 pt
{ "mode": "auto", "padding": 8 }                                                         // 그려 보고 내용 상자 + 여유

// FitOpts — 쪽마다 A4 한 장에 가운데 맞춤(회전된 쪽은 보이는 모양 그대로, /Rotate 0으로)
{ "orientation": "auto" /* auto: 가로로 보이는 쪽은 가로 A4 | portrait | landscape */, "margin": 0, "enlarge": true }

// NupOpts
{ "per_sheet": 2 /* 2|4 */, "orientation": "auto" /* 2쪽=가로, 4쪽=세로 */, "order": "across" /* across | down */, "gap": 8, "margin": 14 }

// ProtectOpts
{ "open_password": "",            // 비우면 누구나 열고 아래 제한만
  "owner_password": "",           // 비우면 무작위(제한을 풀 수 없게)
  "allow_print": true, "allow_copy": true, "allow_edit": true,
  "cipher": "aes256" }            // aes256(기본, 한글 암호 가능) | aes128(영문·숫자 암호만)

// DocInfo — pdf_info_set: 넣은 항목만 바꿈, "" = 그 항목 지움, 없음/null = 그대로. XMP도 같은 값으로 새로 씀
{ "title": "분기 보고서", "author": "홍길동", "subject": null, "keywords": null, "creator": null, "producer": "" }
```

양식 `kind`: `text` | `checkbox` | `radio` | `combo` | `list` | `button` | `signature`. 채우기 값: 글자/선택 칸은 글자, 체크 상자는 `true`/`false`(또는 켜짐 상태 이름), 라디오는 `options` 중 하나(`Off` = 모두 끔). 글자·선택 칸은 채울 때 모양을 직접 만들어(한글 부분 글꼴) 보는 프로그램이 다시 그리지 않게 한다(`NeedAppearances false`). 평평하게는 양식 칸과 모양이 있는 주석을 쪽 내용으로 그려 넣고 `AcroForm`을 지운다(링크는 남김, 숨김 주석·팝업은 버림).

### 구현 메모

- 쪽 편집·뽑기·끼워 넣기·합치기는 `plan::apply` 한 경로(lopdf로 원본별로 열어 페이지 트리를 평평하게 → 객체 번호를 겹치지 않게 → 새 `/Pages`).
- 얹는 내용은 원래 내용을 `q … Q`로 감싼 뒤 위(또는 아래)에 새 내용 스트림을 붙이고, 보이는 쪽 → 사용자 공간 행렬(`PageGeom::visual_to_user`, /Rotate 0·90·180·270과 CropBox 원점 반영)로 그린다.
- A4 맞춤·모아 찍기는 쪽을 Form XObject(BBox = 보이는 상자)로 바꿔 새 종이에 놓는다. 쪽 모양이 바뀌므로 주석·양식은 뺀다(보이는 모양은 남음). A4 맞춤은 같은 쪽 객체를 고쳐 책갈피가 계속 그 쪽을 가리킨다.
- 암호: lopdf 0.46의 표준 보안 처리기(AES-128 = V4/R4 AESV2, AES-256 = V5/R6 AESV3), 파일 키는 `getrandom`. 제한만 걸린 PDF의 보호 풀기는 소유자 암호를 확인한다.
- 미리보기: PDFium `set_target_width` + PNG(빠른 압축). 측정은 보고서(W3) 참고.
## 0.3.0 문서 변환(W2)

Word→PPT, PDF→Word, PDF→PPT, PDF→TXT, 이미지만 뽑기. 모두 순수 Rust(+ 이미 쓰는 PDFium)이고,
**LibreOffice의 PDF 가져오기(xpdfimport/poppler, GPL)는 쓰지 않는다**. LibreOffice는 Word→PPT에서 `.doc/.hwp/.odt/.pages/.rtf`를
먼저 docx로 바꿀 때만(`docs::convert`, W1의 앱 내장 LO를 `Toolset`이 먼저 찾음) 쓴다.

모듈: `core/ooxml/{pptx,docx}.rs`(공용 쓰기 — W3도 씀), `core/docx2pptx.rs`, `core/pdflayout.rs`(PDF 글자 배치 분석, 아래 셋이 공유),
`core/pdf2docx.rs`(PDF→Word, PDF→TXT), `core/pdf2pptx.rs`, `core/officeimages.rs`. 화면 명령은 `src-tauri/src/cmd_office.rs`.

### 화면 명령 계약(Tauri)

공통: `job`은 화면이 정한 번호. 진행률은 이벤트 **`job-progress` `{ job: number, fraction: 0..1 }`**(1% 단위), 멈추기는 기존 **`cancel_jobs`**(돌고 있는 모든 작업).
`out_dir`가 비거나 없으면 원본 옆. 결과는 덮어쓰지 않는다(`이름_PPT (1).pptx`). 실패는 한국어 문장(문자열)으로 reject.
경로 필드는 문자열(Windows 경로).

| 명령 | 인자 | 결과 이름 | 돌려주는 JSON |
|---|---|---|---|
| `doc_to_pptx` | `{ job, path, outDir? }` | `이름_PPT.pptx` | `{ input, output, slides, sections, images, tables, via_libreoffice, millis, note }` |
| `pdf_to_docx` | `{ job, path, outDir? }` | `이름_Word.docx` | `{ input, output, pages, headings, paragraphs, list_items, tables, images, scanned_pages, two_column_pages, dropped_lines, millis, note }` |
| `pdf_to_pptx` | `{ job, path, mode?: "image" \| "editable", dpi?: number, outDir? }` | `이름_PPT.pptx` | `{ input, output, mode, slides, text_boxes, pictures, shapes, image_pages, millis, note }` |
| `pdf_to_txt` | `{ job, path, outDir? }` | `이름.txt` | `{ input, output, pages, chars, scanned_pages, millis, note }` |
| `extract_images` | `{ job, path, outDir? }` | 폴더 `이름_이미지\` | `{ input, folder: string \| null, files: string[], duplicates, skipped, millis, note }` |

- `note`는 사용자에게 그대로 보여 줄 한 줄 요약(예: "3쪽 → 제목 9, 문단 13, 목록 13, 표 1, 그림 2").
- `mode` 기본 `"image"`(그대로 보이게), `"editable"`(글자 고칠 수 있게). `dpi` 기본 150(36~600).
- `extract_images`는 빠르므로 진행 이벤트를 시작(0)·끝(1)만 보낸다. 그림이 없으면 `folder: null`, 폴더를 만들지 않음.
- 받는 파일: `doc_to_pptx` — docx/docm/dotx + LibreOffice로 여는 글 문서(doc, odt, pages, rtf 등). 한글(hwp/hwpx)은 아래 "0.3.0 한글(W6)"의 길을 따른다. LibreOffice가 없으면
  "LibreOffice을(를) 찾지 못했습니다…" 오류. `extract_images` — docx/pptx/xlsx(매크로·서식 포함), pdf, hwp/hwpx(W6).
- 오류 문구 예: 암호 PDF "암호가 걸린 PDF는 열 수 없습니다. PDF 프로그램에서 암호를 풀어 저장한 뒤 다시 시도하세요",
  손상 "PDF 파일이 손상되었거나 PDF 형식이 아닙니다" / "Word 파일을 읽지 못했습니다(…). 파일이 손상되었거나…", 취소 "취소했습니다".

CLI(같은 코어): `filetool-cli doc-to-pptx | pdf-to-docx | pdf-to-pptx [--mode image|editable] [--dpi N] | pdf-to-txt | extract-images [--out 폴더] <파일...>`.

### Word → PPT 규칙

- Title → 첫 슬라이드(+ 바로 뒤 보통 문단 2개는 부제). Heading 1/2 → 한 장씩(H2 위에 상위 H1을 작게). H3 이하 → 굵은 줄.
  제목 스타일은 `w:name`(heading N / 제목 N / title)·`outlineLvl`·`basedOn` 사슬로 찾는다(한국어판 Word의 스타일 ID `1`,`2`,`a3`도 됨).
- 문단·목록 → 글머리표(목록 단계, 번호 목록은 번호). 7줄 또는 420자를 넘으면 고르게 나눠 "(계속)".
- 섹션의 첫 그림은 글 오른쪽, 나머지는 한 장씩. 그림 설명(Caption 스타일 또는 그림·표 바로 뒤 "그림 N…/표 N…")은 그림 아래 작은 글씨.
- 표는 진짜 표. 12줄 넘으면 머리글 줄을 반복해 나눔("(표 1/2)"). 글이 3줄·200자 이하이고 표가 7줄 이하면 글과 표를 한 장에.
- 머리글·바닥글은 넣지 않음. 표 안 그림은 표 다음 슬라이드에. EMF/WMF 그림은 바꾸지 않고 그대로(Content-Type은 내용으로 판단).
  `mc:Fallback` 안(같은 내용의 옛 형식)은 읽지 않는다(두 번 들어가지 않게).
- 슬라이드는 16:9, 자체 템플릿(마스터 1·빈 레이아웃 1·테마 1, 맑은 고딕). 자리 표시자 없이 위치를 정한 글상자·그림·표만.

### PDF 글자 배치 분석(`pdflayout`)

1. PDFium으로 글자마다 위치·크기·굵기·기울임·글꼴·색·링크를 읽는다(보이지 않는 글자 = OCR 층은 뺌).
2. **틀**: 글자 각도의 최빈 사분면으로 좌표를 돌려 가로로 세운다(돌린 쪽 대응). 쪽 /Rotate와 같으면 `upright`(화면과 같은 방향).
3. 기준선이 같은 글자 → 행. 행 안 틈이 글자 크기 1.2배를 넘거나 그 사이에 세로 선이 있으면 칸을 나눈다. 글머리표·번호와 글 사이 틈은 붙인다.
4. **단**: 가운데(25~75%)에 긴 글줄이 거의 지나가지 않는 세로 빈 띠가 8pt 넘고 양쪽에 긴 줄이 4개 이상이면 2단. 가로지르는 줄(제목)이 나오면 그 앞까지 왼쪽 단 → 오른쪽 단 순서.
5. **머리글·바닥글**: 쪽 위·아래 10% 안 줄 중 여러 쪽(3쪽 이상이면 절반 이상)에 되풀이되는 줄(숫자는 같게 봄), 쪽 번호 모양("3", "- 3 -", "3 / 10", "3쪽", "Page 3"),
   끝에 쪽 번호가 붙은 줄("… · 3 쪽")은 뺀다. 한 쪽짜리 PDF의 쪽 번호 없는 머리글은 알 수 없어 남는다.
6. **표**: 칸 2개 이상 줄이 2줄 넘게 이어지면 표. 세로 선이 표 높이 대부분에 걸치면 선 사이가 열(가운데 정렬 머리글도 맞음), 선이 없으면 칸의 가로 범위가 겹치는 것끼리 한 열.
   가로 선이 있으면 선 사이 여러 줄을 한 행으로. 첫 줄이 굵거나 색 바탕이면 머리글 줄. 다음 쪽 첫 표가 열 수가 같으면 이어 붙인다.
7. **제목**: 한 칸 줄이고 본문(글자 수로 가중한 최빈 크기)보다 15% 크거나, 굵고 짧은(60자 미만, 마침표로 안 끝남) 줄. 크기 순위로 단계, 가장 큰 크기가 첫 쪽에 한 번만 있고 본문의 1.4배 이상이면 Title.
8. **문단**: 같은 크기·같은 왼쪽·좁은 줄 간격이고 앞줄이 오른쪽 끝 가까이 찼으면 잇는다. 쪽을 넘거나(앞 쪽 아래 → 다음 쪽 위) 단을 넘는(왼쪽 단 끝 → 오른쪽 단 첫 줄) 문단도 잇는다.
   **한글 줄 잇기**: 줄 끝 빈칸을 글자로 남기는 PDF(LibreOffice·Word)는 빈칸이 없으면 낱말 안에서 끊긴 것 → 빈칸 없이 붙인다(`지적\n된` → `지적된`).
   줄 끝 빈칸을 남기지 않는 PDF(Edge/Chrome 인쇄)는 앞줄이 오른쪽 끝까지 찼을 때만 붙인다. 영어는 빈칸, 줄 끝 하이픈은 이어 붙임.
9. **목록**: 글머리표 글자(•, ▪, -, 사설 영역 기호 등), 번호("1." "(2)" "가." "a)"), 줄 왼쪽에 도형으로 그린 점(Edge). 들여쓰기가 깊으면 아래 단계(최대 3).
   앞줄이 끝까지 찬 같은 왼쪽 줄의 "다." "가." 등은 번호가 아니라 이어지는 글.
10. **스캔본**: 글자 20자 미만 + 쪽 절반 넘는 그림(또는 도형만 60개 넘음) → 150dpi JPEG 쪽 그림. OCR은 하지 않는다.
11. **글꼴 표**(`map_font`): 맑은 고딕·나눔고딕·Noto Sans·본고딕 등 고딕 → 기본(맑은 고딕), 명조·바탕·Noto Serif → 바탕, 굴림·돋움·궁서 → 같은 이름,
    Times/Liberation Serif → Times New Roman(한글은 바탕), Arial/Helvetica/Liberation Sans → Arial, Calibri/Carlito → Calibri, Cambria/Caladea → Cambria,
    Courier/Consolas/D2Coding → Courier New. docx의 `fontTable.xml`에 글꼴 계열을 적어 그 글꼴이 없는 PC도 비슷한 계열로 그린다.

### PDF → Word / TXT / PPT

- **Word**: 위 블록 → Title/Heading1~3, 문단, 목록(ListBullet/ListNumber, 목록마다 번호 1부터), 표(TableGrid, 머리글 줄 반복), 그림(JPEG는 원본 바이트), 캡션,
  굵게·기울임·하이퍼링크, 쪽 크기 = 첫 쪽 크기(여백 2cm). 스캔본 쪽은 앞뒤 쪽 나누기 + 쪽 그림. 여러 단은 한 단으로 흘린다(단 편집은 옮기지 않음).
- **TXT**: UTF-8 **BOM** + CRLF(메모장). 문단은 한 줄, 목록은 "• " / "1. ", 표는 칸을 탭으로. **쪽 사이: 빈 줄 + `— N쪽 —` + 빈 줄**
  (폼 피드는 메모장에서 이상한 기호로 보여서 쓰지 않음). 스캔본 쪽은 "(글자가 없는 쪽입니다 — 스캔한 그림)".
- **PPT `image`**(기본): 쪽마다 PDFium 렌더(150dpi JPEG 품질 90) 한 장. 슬라이드 크기 = 첫 쪽 크기(PowerPoint 범위 1~56인치로 맞춤), 다른 크기 쪽은 가운데 맞춤.
- **PPT `editable`**: 같은 분석으로 줄을 제자리 글상자로(같은 크기·왼쪽·촘촘한 줄은 한 상자, 줄은 그대로·줄 간격 고정·줄바꿈 없음), 칸이 나뉜 줄은 칸마다 상자,
  그림은 제자리, 칠한 사각형·선은 도형(최대 600개, 흰 사각형·쪽 크기 배경은 뺌). 스캔본 쪽과 글자가 화면과 다른 방향인 쪽은 쪽 그림.

### 이미지만 뽑기

- docx/pptx/xlsx: 문서에서 처음 쓰인 순서(Word 본문 → 머리글·바닥글, PowerPoint는 `presentation.xml`의 슬라이드 순서, Excel은 시트 순서 → drawing),
  차트·다이어그램 안쪽도 한 단계 더 따라감. 어디에도 안 쓰인 그림(마스터·레이아웃)은 맨 뒤. 같은 바이트는 한 번만. 이름 `NN_원래이름`(`01_image3.png`), 원본 바이트 그대로.
- PDF: 1c `pdfrender::extract_images`와 같은 방식(JPEG 그대로, 8비트 Flate RGB/흑백은 PNG, 그 밖은 건너뛰고 개수만)이고 폴더만 `이름_이미지\`.

### 시험 샘플(`crates/core/tests/data/030/`)

`make_corpus.py`(python-docx·python-pptx·openpyxl·pypdf·Pillow + LibreOffice로 PDF 만들기)로 만든 한국어 샘플. 글·그림은 스크립트가 지어낸 것,
PDF에 들어간 글꼴은 나눔고딕·Noto(SIL OFL) 일부. `엣지인쇄.pdf`만 Windows Edge `--headless --print-to-pdf`로 `엣지인쇄.html`을 인쇄해 만든다(Noto Sans KR).
`tests/office_030.rs`가 구조 숫자(제목·문단·목록·표·그림·글상자·슬라이드 수, 한글 낱말 사이 빈칸)를 확인한다(픽셀 비교 아님).

## 0.3.0 이미지·영상·파일 정리 (W4)

사진 고치기·영상 고치기·zip 풀기/만들기·이름 한꺼번에 바꾸기·같은 파일 찾기. 코어 모듈: `core/imgedit.rs`, `core/mediaedit.rs`,
`core/ziptool.rs`, `core/rename.rs`, `core/dupes.rs`(+ `exif::has_gps`, `heif::HeifInfo::display_size`, `samples::write_windows_cp949_zip`).
화면 명령은 `src-tauri/src/cmd_tools.rs`, CLI는 `filetool-cli help-tools`(`crates/cli/src/tools030.rs`, 결과는 같은 JSON).

### 공통 규칙

- 경로는 문자열(Windows 경로). `outDir`가 비거나 없으면 원본 옆. 결과는 덮어쓰지 않는다(`이름_편집 (1).jpg`). 원본은 그대로.
- **진행·멈추기**: 오래 걸리는 명령(`image_edit`, `media_edit`, `zip_extract`, `zip_create`, `dup_scan`)은 `job?: number`(화면이 정한 번호)를 받는다.
  주면 이벤트 **`tools_progress` `{ job, done: number, total: number, current: string }`**를 보내고, **`cancel_job { job }`**(그 작업만, 있었으면 `true`)
  또는 예전 `cancel_jobs`(모두)로 멈춘다. `done/total`: 여러 파일이면 파일 수(current = 지금 파일), `media_edit`는 0~1 비율(total = 1),
  `dup_scan`은 해시를 잰 파일 수(목록 만들기 단계에서는 total = 0).
- **오류**: `{ code, message }` — `code` = `cancelled` | `tool_missing`(ffmpeg 없음) | `error`. `message`는 그대로 보여 줄 한국어 문장.

### 명령 계약(Tauri)

| 명령 | 인자 | 돌려주는 JSON |
|---|---|---|
| `image_probe` | `{ path }` | `{ width, height, format: "jpg"\|"png"\|"webp"\|"heic"\|"bmp"\|"tif"\|"gif", size, has_gps, taken_at? }` |
| `image_edit` | `{ inputs: string[], ops, outDir?, job? }` | `[{ input, output: string\|null, before, after, width, height, note?, error? }]` |
| `media_probe` | `{ path }` | `{ duration, width, height, rotation, has_audio, video_codec: string\|null, audio_codec: string\|null }` |
| `media_frame` | `{ path, at: number, width? }` | `string` — JPG 바이트의 base64(앞에 `data:image/jpeg;base64,`를 붙여 `<img>`에) |
| `media_frame_save` | `{ path, at, outDir? }` | `string` — 저장한 `이름_장면_mm-ss.jpg` 경로 |
| `media_edit` | `{ input, ops, outDir?, job? }` | `{ output, before, after, duration, reencoded, note? }` |
| `zip_list` | `{ path, encoding?: "auto"\|"utf8"\|"cp949" }` | `{ entries: [{ name, size, is_dir }], detected_encoding: "utf8"\|"cp949", garbled_guess }` |
| `zip_extract` | `{ path, encoding?, outDir?, job? }` | `{ folder, count, skipped: [{ name, reason }] }` |
| `zip_create` | `{ inputs: string[], output?, job? }` | `{ output, count, size }` |
| `rename_preview` | `{ files: string[], rule, sort?: "name"\|"modified"\|"taken" }` | `[{ from, to, conflict?, note? }]` |
| `rename_apply` | `{ pairs: [{ from, to }] }` | `{ done, failed: [{ from, reason }], undo_id: string\|null }` |
| `rename_undo` | `{ undoId }` | `{ done, failed: [{ from, reason }] }` |
| `dup_scan` | `{ roots: string[], minSize?, includeHidden: bool, job? }` | `{ groups: [{ size, hash, files: [{ path, modified }] }], scanned, wasted_bytes }` |
| `dup_trash` | `{ paths: string[] }` | `{ done, failed: [{ path, reason }] }` |

크기는 바이트(`before`·`after`·`size`), 시간은 초(`duration`·`at`), `modified`는 유닉스 밀리초. Tauri 인자 이름은 camelCase(`outDir`, `undoId`,
`minSize`, `includeHidden`), `ops`·`rule` 안의 필드는 아래처럼 snake_case 그대로.

**`image_edit` ops** — `{ resize?: { width?, height?, percent?, keep_ratio }, crop?: { x, y, w, h } | { aspect: "1:1"|"4:3"|"16:9"|"3:4"|"9:16", anchor: "center" },
rotate?: 90|180|270, flip?: "h"|"v", strip_meta: bool, format?: "keep"|"jpg"|"png"|"webp", quality?: 1..100, target_kb? }`
- 순서: EXIF 회전을 픽셀에 적용 → 자르기(보이는 그대로의 좌표) → 돌리기(시계 방향) → 뒤집기 → 크기 → 저장. 결과 이름 `이름_편집.확장자`.
- `resize`: `percent`가 있으면 그것. `width`만/`height`만이면 나머지는 비율대로(`keep_ratio`). 둘 다면 `keep_ratio`일 때 그 상자 안에 들어가게, 아니면 그 크기 그대로.
- `strip_meta`: EXIF(GPS·카메라·촬영 날짜)·XMP·IPTC를 넣지 않는다. 고칠 것이 정보 지우기뿐이고 회전 값이 없는 JPEG는 다시 압축하지 않고 조각만 뺀다
  (`note`: "다시 압축하지 않아 화질이 그대로입니다"). `strip_meta: false`면 EXIF를 옮기되 회전 값은 1(이미 픽셀에 적용)로 — JPG/PNG만.
- `format: "keep"`(기본): 원본 형식. HEIC는 JPG로(HEIC는 ffmpeg로 푼다 — `imageconv::load_heif`). `quality` 기본 90.
- `target_kb`(1KB = 1024바이트): JPG/WebP는 품질을 이분 탐색(12~95)해 목표 이하에서 가장 높은 품질, 가장 낮은 품질로도 크면 크기를 줄여 다시.
  PNG 등 무손실은 크기만 줄인다. 결과는 목표 이하(시험 사진 1600×1200: 300KB 목표 → 99.8%, 120KB → 93.7% — 품질이 정수 단계라 딱 맞지는 않음). 못 맞추면 가장 작게 만든 것 + `note`("목표 …KB까지 줄이지 못해…").
- 한 장이 실패해도 나머지는 계속(그 항목은 `output: null`, `error`).

**`media_edit` ops** — `{ trim?: { start, end }, mute?: bool, rotate?: 90|180|270 }`(시계 방향). 결과 `이름_편집.확장자`.
- 다시 압축하지 않음(`reencoded: false`, 빠르고 화질 그대로): MP4/MOV/M4V 영상, MP3/M4A 소리.
  자르기는 시작점 바로 앞의 키프레임(최대 1초 일찍)에서 시작 — `note`에 "가까운 장면 전환점에 맞춰 0.4초 일찍(0:01.2) 시작합니다".
  1초 안에 키프레임이 없으면 다시 압축해 정확히 자름. 돌리기는 회전 정보만 바꿈(기존 회전 + 요청).
- 그 밖(MKV·AVI·WebM·WMV·WAV…)은 앱에 든 LGPL ffmpeg로 다시 압축: 영상 → MP4(H.264 `h264_mf`, libx264 있는 ffmpeg면 libx264 / AAC 160k), 소리 → M4A.
  이때 돌리기는 픽셀을 돌린다(transpose).
- 소리만 있는 파일에서 `mute`·`rotate`, 소리 없는 영상에서 `mute`는 쉬운 말 오류.
- `media_probe`: `width/height`는 보이는 크기(회전 반영), `rotation`은 플레이어가 돌려 보여 주는 시계 방향 각도. 표지 그림(mp3 앨범 그림)은 영상으로 치지 않음.
- `media_frame`/`_save`: 길이보다 뒤를 고르면 끝 장면(저장 이름도 그 시각). 회전은 반영된 그림.

**zip** — `encoding: "auto"`(기본): UTF-8 표시(bit 11)가 없는 이름 중 UTF-8로 안 읽히는 것이 있으면 CP949(윈도 탐색기 "압축(ZIP) 폴더"), 아니면 UTF-8.
UTF-8 표시가 있는 이름은 언제나 UTF-8. 화면은 이름이 이상하면 `utf8`/`cp949`를 골라 `zip_list`를 다시 부르면 된다.
`garbled_guess`: UTF-8 표시 없이 한글 등 ASCII 밖 이름이 든 zip(다른 프로그램·다른 OS에서는 이름이 깨져 보였을 수 있음).
- 풀기: zip 옆(또는 `outDir`)에 새 폴더 `이름\`(있으면 `이름 (1)\`). 이름은 NFC, 윈도에서 못 쓰는 글자 `<>"|?*`는 `_`, 끝의 점·빈칸은 지움, `CON` 등은 앞에 `_`.
  `skipped.reason`: "맥이 만든 부속 파일이라 뺐습니다"(`__MACOSX/`, `._*`, `.DS_Store`), "폴더 밖을 가리키는 이름이라 뺐습니다"(`..`·절대 경로·드라이브),
  "같은 이름이 이미 있어 덮어쓰지 않았습니다", "암호가 걸린 항목이라 풀지 못했습니다", "바로 가기(링크)라 뺐습니다". 수정 시각은 zip 안 시각으로.
- 만들기: `output`이 없으면 첫 항목 이름 + `.zip`(겹치면 ` (1)`). 폴더는 그 폴더 이름부터 담는다. 이름은 NFC + UTF-8 표시.
  `.DS_Store`·`._*`·`Thumbs.db`·`desktop.ini`는 뺌, 이미 압축된 형식(jpg·png·heic·mp4·mov·mp3·zip·docx…)은 압축 없이 담음. `count` = 담은 파일 수(폴더 제외).
- 예전 화면의 `zip_extract { zip, outDir, rezip }`도 그대로 받는다(맥 zip NFC + 다시 압축, 예전 결과 `{ extract, rezip }`). 새 화면은 `path`를 쓴다.

**이름 바꾸기 rule** — `{ find?, replace?, regex: bool, prefix?, suffix?, numbering?: { start, step, digits, position: "prefix"|"suffix"|"replace" },
date?: { source: "taken"|"modified", format: "YYYY-MM-DD"|"YYYYMMDD"|"YYYY-MM-DD_HHmmss", position: "prefix"|"suffix" }, case?: "lower"|"upper"|"keep", ext?: "keep"|"lower" }`
- 확장자 앞 이름에 적용. 순서: 찾아 바꾸기 → 대소문자 → 번호 `replace` → 날짜·번호(앞/뒤, `_`로 이음) → `prefix`·`suffix`(그대로 붙임) → 확장자.
  예: `prefix: "여행_"`, 번호 앞 3자리, `IMG_` 지우기 → `IMG_1.JPG` → `여행_001_1.jpg`(`ext: "lower"`).
- `sort`(기본 `name`, 사람 순서: 사진2 < 사진10)가 번호 순서. `taken`은 EXIF 촬영 날짜(없으면 수정 날짜) — 그때 `note`.
- `conflict`(있으면 적용에서 빼야 함): "다른 파일과 새 이름이 겹칩니다", "같은 이름의 파일이 이미 있습니다", "이름에 쓸 수 없는 글자…", "이름 끝에 점이나 빈칸…",
  "윈도가 쓰는 이름…". 대소문자만 다른 이름은 같은 이름으로 본다(윈도).
- `rename_apply`: 모두 임시 이름(`.ft-rename-…tmp`)으로 → 새 이름으로(맞바꾸기 `a→b, b→a`·이어 바꾸기 됨). 새 이름이 이미 있으면(목록 밖 파일) 그 항목은 원래 이름으로
  돌려 두고 `failed`. 되돌리기 기록은 앱 설정 폴더 `rename-undo\<undo_id>.json`(바꾼 것이 없으면 `undo_id: null`).
- `rename_undo`: 기록대로 되돌리고 기록을 지운다(일부 실패면 남은 것만 기록에 둠). 두 번째는 "되돌릴 기록이 없습니다" 오류.

**같은 파일** — `dup_scan`: 크기가 같은 것 → 앞·뒤 64KiB SHA-256 → 전체 SHA-256이 같은 것끼리 묶음. 0바이트·링크는 보지 않음,
`includeHidden: false`면 `.`으로 시작하는 이름과 숨김·시스템 속성 파일·폴더를 뺌. `minSize` 기본 1. 겹치는 루트는 한 번만.
`groups`는 비울 수 있는 크기가 큰 순, 묶음 안은 경로 순. `scanned` = 살펴본 파일 수.
- `dup_trash`: **휴지통으로만**(Shell `SHFileOperationW` + `FOF_ALLOWUNDO`, `FOF_WANTNUKEWARNING`). 휴지통이 없는 드라이브(USB·네트워크)는 거절.
  마지막 `dup_scan` 결과로 확인해, 어떤 묶음이든 한 벌도 남지 않게 되는 요청은 **통째로** 거절("같은 파일 N개를 모두 지우게 되어…").
  결과에 없는 파일, 남길 파일이 그사이 바뀌었거나 없어진 경우도 거절. 보내기 직전 그 파일의 크기·해시가 찾을 때와 같은지 다시 확인.

### 앱에 든 ffmpeg(LGPL) — W4가 쓰는 것

`scripts/build-ffmpeg.sh`(W1)에 이미 들어 있는 것만 쓴다(빌드 바꾸지 않음): `-ss/-t`, `-c copy`, `-display_rotation`, `-an`, 필터 `transpose`·`hflip`·`vflip`·`scale`,
인코더 `mjpeg`(화면 한 장)·`h264_mf`·`aac`, 먹서 `mp4`·`mov`·`ipod`(m4a)·`mp3`·`image2`. MKV·WebM 먹서는 없으므로 그 형식은 MP4로 다시 만든다.

### 새 의존성

| 크레이트 | 쓰는 곳 | 라이선스 |
|---|---|---|
| `regex` 1.13 (이미 Cargo.lock에 있던 판) | 이름 바꾸기 정규식 | MIT OR Apache-2.0 |
| `sha2` 0.10.9 (이미 Cargo.lock에 있던 판) | 같은 파일 해시 | MIT OR Apache-2.0 |
| `serde`, `serde_json`(CLI에 추가, 코어엔 이미 있음) | CLI JSON 출력 | MIT OR Apache-2.0 |
| `static_vcruntime` 3.0 (빌드 전용, 이미 Cargo.lock에 있던 판 — tauri-build가 씀) | CLI·서비스 VC 런타임 정적 링크 | MIT OR Apache-2.0 OR Zlib |

### Windows C 런타임

`filetool-cli.exe`·`filetool-svc.exe`는 `build.rs`에서 `static_vcruntime::metabuild()`(MIT OR Apache-2.0 OR Zlib) — VC 런타임(`VCRUNTIME140.dll`)을
exe 안에 넣고 UCRT(Windows 10부터 OS에 있음, `api-ms-win-crt-*`)는 동적으로 둔다. `win-filetool.exe`는 tauri-build가 이미 같은 방식.
`-C target-feature=+crt-static`을 전체에 걸면 tauri-build의 `/NODEFAULTLIB:libucrt.lib`와 부딪혀 앱이 링크되지 않으므로 쓰지 않는다.
`llvm-readobj --coff-imports`로 세 exe에 `VCRUNTIME140`·`MSVCP140`이 없는지 확인한다.
## 0.3.0 한글(W6)

스파이크 결론: LibreOffice(전체·축소 모두)는 **한글 97 이전만** 읽는다. 한글 2002 이후(HWP 5.x) 4/4, HWPX 2/2는 "source file could not be loaded".
그래서 형식을 내용으로 구분해 **되는 것만 보여 주고**, 한글 프로그램이 설치된 PC에서는 한글 자체로 바꾼다.

모듈: `core/hwp/{mod,detect,hancom,images}.rs`(+ `hancom.ps1`), 화면 명령 `src-tauri/src/cmd_hwp.rs`, CLI `crates/cli/src/hwp030.rs`.

### 형식 알아보기(`hwp::detect`)

| 형식 | 판단 | 표시 |
|---|---|---|
| `hwp97` | 앞 `HWP Document File V…`(V3.00 등) | 버전 `3.00` |
| `hwp5` | CFB 서명 + `/FileHeader` 서명 `HWP Document File` | 버전 `5.1.0.1`, 속성 비트 0 압축 · 1 암호 · 2 배포용 · 4 DRM · 8 인증서 암호화 · 10 인증서 DRM |
| `hwpx` | zip + `mimetype` = `application/hwp+zip`(또는 `Contents/content.hpf`) | `version.xml`, `META-INF/manifest.xml`에 `encryption-data`면 암호 |
| `damaged` | 그 밖(빈 파일, CFB인데 FileHeader 없음, 깨진 zip, 일반 zip 등) | |

확장자 `.hwp`/`.hwpx`는 모두 문서(`docs::family` = Text)로 받고, 실제로 바꿀 수 있는지는 형식으로 정한다(`docs::allowed_targets` → `hwp::doc_targets`).

### 할 수 있는 것과 길

| 형식 | 한글 설치됨 | 한글 없음 |
|---|---|---|
| 한글 97 | PDF·DOCX(→PPT): `hancom` | PDF·DOCX(→PPT): `libreoffice`(앱에 든 LO). 그림 뽑기 안 됨 |
| 한글 5·HWPX | PDF·DOCX(→PPT): `hancom`, 그림 뽑기 | PDF·PPT 안 됨 → 안내, 그림 뽑기만 |
| 암호·DRM·배포용·손상 | 모두 안 됨(이유 안내) | 모두 안 됨(이유 안내) |

기존 변환 명령은 이름·인자가 그대로이고 코어에서 길을 고른다: 문서 → PDF/DOCX(`convert_files`의 `doc` 작업, 감시 폴더 포함)는 `docs::convert` → `hwp::convert`,
Word → PPT(`doc_to_pptx`)는 한글이면 `hwp::convert(… Docx)`로 docx를 먼저 만든 뒤 같은 규칙, 이미지만 뽑기(`extract_images`)는 `hwp::images`.
안 되는 조합은 아래 안내 문장 그대로 오류(reject)한다.

### 화면 명령 계약

| 명령 | 인자 | 돌려주는 JSON |
|---|---|---|
| `hwp_capabilities` | `{ path }` | `{ format: "hwp97"\|"hwp5"\|"hwpx"\|"damaged", version?, encrypted, distribution, can_pdf, can_pptx, can_images, route: "hancom"\|"libreoffice"\|"none", message? }` |

- 빠르다(파일 앞부분·zip 목록·레지스트리만 읽음, 진행 이벤트 없음). 한글 파일이 아니면 "한글 문서(.hwp, .hwpx)가 아닙니다" 오류.
- 화면은 `can_pdf`/`can_pptx`/`can_images`가 false인 기능을 숨기거나 끄고, `message`를 그대로 보여 준다.
- `extract_images` 결과 JSON에 `skipped_items: string[]`(건너뛴 항목과 이유, 예 `"BIN0003.OLE: OLE 개체(그림이 아님)"`)가 더해졌다(다른 형식은 빈 배열).

안내 문장(`message`·오류):

| 경우 | 문장 |
|---|---|
| 한글 5·HWPX, 한글 없음 | 이 한글 문서(2002년 이후 형식)는 한글 프로그램이 설치된 PC에서만 PDF·PPT로 바꿀 수 있어요. 그림 뽑기는 쓸 수 있어요. |
| 한글 97(그림 뽑기) | 한글 97 문서는 그림 뽑기를 할 수 없어요. PDF로 바꾼 뒤 PDF에서 그림을 뽑아 주세요. |
| 암호·DRM | 암호가 걸렸거나 보호된 한글 문서라 열 수 없어요. 한글에서 암호를 풀어 저장한 뒤 다시 해 주세요. |
| 배포용 | 배포용 한글 문서라 바꾸거나 그림을 꺼낼 수 없어요. |
| 손상 | 한글 파일이 손상되어 읽을 수 없어요. |
| 한글이 멈춤 | 한글 실행 실패: 한글이 응답하지 않아 멈췄어요(시간 초과). 한글 창이나 알림이 떠 있으면 닫고 다시 해 주세요. |

### 설치된 한글로 바꾸기(`hwp::hancom`, Windows)

- 찾기: `HKCR\HWPFrame.HwpObject\CLSID` → `HKLM\SOFTWARE\Classes\CLSID\{…}\LocalServer32`(64비트 보기, 없으면 WOW6432Node)의 Hwp.exe가 있으면 설치됨.
  시험용 `WIN_FILETOOL_HIDE_HANCOM=1`이면 없는 것처럼.
- 실행: 앱이 작업 임시 폴더에 원본을 `in.hwp(x)`로 복사하고 `hancom.ps1`(앱 안에 든 스크립트, UTF-8 BOM)을 한글과 같은 비트의
  `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File`로 실행(콘솔 창 없음). 스크립트가 `HWPFrame.HwpObject`를 만들어
  `RegisterModule` → 창 숨김 → `SetMessageBoxMode(0x211411)` → `Open(in, "", "forceopen:true;suspendpassword:true;versionwarning:false;lock:false")`
  → `SaveAs(out, "PDF" | "OOXML")` → `Clear(1)` → `Quit()`. 상태 줄(`PID n`, `REG`, `OPEN`, `PAGES n`, `SAVE`, `ERROR …`, `DONE`)을 앱이 읽는다.
  결과는 원본 옆 `이름.pdf`/`이름.docx`(겹치면 ` (1)`), 원본 수정 시각을 따른다.
- 파일 경로 보안 창 없애기: 한글은 자동화로 파일을 열거나 저장할 때 "접근 허용" 창을 띄운다(실측: 창이 떠서 멈춤). 한컴이 안내하는 방법은
  보안 모듈 DLL을 `HKCU\Software\HNC\HwpAutomation\Modules`에 등록하고 `RegisterModule("FilePathCheckDLL", 값 이름)`을 부르는 것.
  한컴 예제 DLL(FilePathCheckerModuleExample)은 재배포하지 않는다. 예제와 같은 내보내기(`IsAccessiblePath`, stdcall 인자 16바이트, 항상 1)를 가진
  1 KB짜리 32비트 DLL을 `hancom::path_check_dll()`이 코드로 만들어 `%LOCALAPPDATA%\kr.salarycrew.winfiletool\hwp\WinFileToolHwpPath-<해시>.dll`에 쓰고
  값 `WinFileToolPathCheck`로 등록한다(가져오는 함수·재배치 없음, DllMain은 TRUE만). 같은 사용자 권한의 프로그램은 이 레지스트리를 원래 쓸 수 있어 새 권한은 생기지 않는다.
  0.3.0 출시: 등록값은 **변환하는 동안만** 둔다(변환 잠금을 잡은 뒤 등록 → 끝나면 성공·실패·취소 모두 값 삭제, 키는 남김).
  설치본을 지우면 훅이 `…\hwp` 폴더와 남은 값(변환 중 강제 종료 대비)을 지운다.
- 한 번에 하나: `…\hwp\hancom.lock` 파일 잠금(앱 창·감시 폴더·CLI 함께). 시간 제한 180초, 취소 가능.
- 끝내기: 스크립트가 알려 준 새 Hwp.exe 번호만(없으면 시작 뒤 새로 생긴 것) 5초 기다렸다가 남아 있으면 `taskkill /F`. 새 프로세스가 생기지 않으면
  (사용자가 쓰는 한글에 붙었을 수 있음) 아무것도 하지 않고 "열려 있는 한글 창을 모두 닫고 다시 해 주세요".
- 메시지 상자 자동 응답(작업 대상은 임시 복사본뿐): 확인 / 확인·취소 → 확인 / 중단·재시도·무시 → 무시 / 예·아니오·취소 → 예 / 예·아니오 → 예 / 재시도·취소 → 취소.
  DOCX 저장은 "서식이 바뀔 수 있음" 확인 창이 나오므로 '확인'이어야 저장된다(실측: 취소로 두면 `SaveAs` false).

### 그림 뽑기(`hwp::images`)

- 한글 5: `DocInfo`(문서 압축이면 raw DEFLATE) 레코드 중 BIN_DATA(태그 0x12) 순서대로. 종류 묻음 → `BinData/BIN%04X.확장자`,
  **항목별 압축 설정**(문서 따름/압축/압축 안 함)을 따라 풂. 링크(문서 밖 파일)·OLE 저장소는 건너뛰고 이유를 남김. DocInfo에 목록이 없으면 BinData 이름순.
- HWPX: `Contents/content.hpf`의 `opf:item` 중 `BinData/` 순서 → manifest에 없는 `BinData/` 이름순.
- 그림 판단은 앞부분 서명(PNG·JPEG·GIF·BMP·TIFF·WebP·WMF(placeable/표준)·EMF). WMF/EMF는 원본 파일 그대로. 그 밖은 "그림 형식이 아님"으로 건너뜀.
- 이름 `NN_BIN0001.jpg`(한글 5) / `NN_image1.png`(HWPX, 이름은 마지막 부분만·예약 문자 제거), 폴더 `이름_이미지\`, 같은 바이트는 한 번만(W2와 같음).
- 한도: 항목 256 MB, 합계 2 GB(압축 푼 뒤), 항목 2만 개, DocInfo 64 MB. 넘으면 그 항목만 건너뜀(압축 폭탄 시험 있음). `..`·`\`·`:`·빈 경로 조각이 든 zip 이름은 건너뜀.
- 암호·DRM·배포용·손상·한글 97은 위 안내 문장 오류. 그림 없는 정상 문서는 오류 아님(폴더 만들지 않음).

### 의존성

`cfb` 0.14(MIT, 이미 Cargo.lock에 있던 판 — tauri의 `infer`가 씀; 하위 `fnv` MIT/Apache, `uuid` MIT/Apache, `web-time` MIT/Apache),
`flate2` 1.1(MIT/Apache, 이미 zip이 씀 — miniz_oxide 백엔드). 새 GPL 없음. 한컴오피스는 사용자 PC에 설치된 것을 COM으로 부를 뿐 앱에 넣지 않는다.

### 시험

코어 단위 시험이 작은 파일을 그 자리에서 만든다(한글 5 CFB + DocInfo + BinData, HWPX zip + manifest): 형식·플래그, 할 수 있는 것 표,
그림 뽑기(항목별 압축, OLE·링크·그림 아님·없는 데이터, 같은 그림, WMF), 크기·개수·합계 한도와 압축 폭탄, 경로 빠져나가기, 보안 모듈 DLL 구조.
`tests/hwp_030.rs`는 LibreOffice 시험 자료의 한글 97 파일(MPL-2.0, `tests/data/030/hwp/README.md`)로 LO 길을 확인한다(LO가 있을 때 변환까지).
정부 배포 실제 문서(HWP 4 + HWPX 2)는 저장소에 넣지 않고 Windows 시험 PC에서 CLI(`hwp-info`, `hwp-convert`, `doc-to-pptx`, `hwp-images`)로 점검했다.

## 0.3.2 핵심 6개 + 우클릭 메뉴

방향: "잡다한 기능은 필요 없다. 진짜 필요한데 간단하고 확실한 기능을." 새 기능 영역은 넣지 않고, 자주 쓰는 6가지를 첫 화면에 두고 나머지는 [더 보기]로 옮겼다(지운 것 없음).

- **화면**: 왼쪽 메뉴는 홈 · 더 보기 · 설정. 홈 = 큰 단추 6개 + 끌어다 놓기(놓은 파일 종류에 맞는 일을 `src/screens/core.ts` `suggest()`가 고름).
  할 일 화면은 하나(`src/screens/job.ts`)를 6가지가 같이 쓴다: 파일 목록 → 설정 한두 개 → 실행 → 한 줄 요약(`summary()`, "10개 중 9개 완료, 1개는 암호가 걸려 있어요") + [폴더 열기].
  예전 화면(변환·PDF 편집·파일 정리·검색·감시 폴더·앱 정보)은 [더 보기]에서 그대로 연다.
- **PDF 용량 줄이기(목표 크기)**: `pdfcompress::compress_to_target`. 단계표 `TARGET_LADDER`(150dpi·품질 75 → 50dpi·품질 30)를 차례로 해 보고,
  목표의 2배를 넘으면 두 단계씩 건너뛴다. 목표는 10진 MB(공공기관 "5MB 이하" = 5,000,000바이트로 보는 편이 안전). 글자·선은 건드리지 않는다.
  끝까지 안 되면 가장 작은 결과를 남기고 `reached=false`로 얼마까지 줄었는지 알린다. 원본이 이미 목표 이하면 새 파일을 만들지 않는다.
- **사진 → PDF(compact)**: `Img2PdfOpts.compact`. A4에서는 찍히는 크기 기준 200 DPI를 넘는 사진을 줄이고, 사진 같은(표본 64×64에서 색 1024개 이상) PNG·WEBP는 JPEG로 넣는다.
  색이 적은 화면 갈무리·도표는 무손실 그대로. 예전 화면(변환 > PDF 하나로)은 compact를 끈 채 예전과 같다.
- **사진 줄이기·JPG로**: `corejobs::photo_shrink`(앱·점검 CLI 공용). 4장씩 동시에, 원본이 JPG이고 결과가 더 크면 버리고 "그대로". 멈추면 만든 파일을 지운다.

### 우클릭 메뉴(`crates/core/src/ctxmenu.rs`)

- 고전 메뉴(정적 동사, 레지스트리)만 쓴다: `HKCU\Software\Classes\SystemFileAssociations\.<확장자>\shell\WinFileTool`(폴더는 `Directory\shell\WinFileTool`)에
  `MUIVerb=파일도구`, `SubCommands=""`, 아래 `shell\<n><할 일>\command` = `"<exe>" --ctx=<할 일> -- "%1"`, `MultiSelectModel=Player`(15개 제한 없음).
  HKCU만 쓰므로 관리자 권한이 필요 없다. 바꾼 뒤 `SHChangeNotify(SHCNE_ASSOCCHANGED)`.
- **Windows 11 첫 메뉴에 올릴 수 있나**: Windows 11의 새 메뉴는 `IExplorerCommand`를 구현한 COM 서버가 **패키지 ID**를 가진 앱(MSIX, 또는 외부 위치 sparse package)에
  `desktop4:FileExplorerContextMenus`로 선언돼 있어야 보인다. 패키지는 서명돼 있어야 설치되고(자체 서명이면 사용자가 인증서를 신뢰 저장소에 넣어야 함 — 관리자 권한),
  레지스트리만으로 첫 메뉴에 넣는 공식 방법은 없다. 서명 인증서가 없는 이 앱은 고전 메뉴만 쓰고, Windows 11에서는 "더 많은 옵션 표시"(Shift+F10·Shift+우클릭) 안에 나온다.
  나중에 코드 서명을 하게 되면 sparse package + `IExplorerCommand` DLL로 첫 메뉴를 더할 수 있다(고전 메뉴는 그대로 둠).
- **여러 파일**: 고전 메뉴는 고른 파일마다 exe를 한 번씩 실행한다. 앞에 떠 있는 앱이 있으면 새 프로세스는 Tauri를 띄우기 전에 single-instance 플러그인과 같은
  WM_COPYDATA(창 클래스 `kr.salarycrew.winfiletool-sic`)로 인자만 넘기고 바로 끝난다(`forward_ctx_fast`). 받는 쪽은 `Collector`에 모았다가
  마지막 인자가 온 뒤 0.7초 조용하면 한 묶음으로 화면에 넘긴다(`ctx_take`, 이벤트 `ctx-ready`). 늦게 온 것은 같은 할 일 화면에 이어 붙인다.
- **보안**: exe 경로와 `%1`은 따옴표로 감싼다. `--` 뒤 인자는 모두 경로로만 보고(옵션으로 읽지 않음 — `--autostart`·`--tab=` 같은 글자가 파일 이름이어도),
  절대 경로가 아니거나 없는 경로는 버린다. 셸(cmd)을 거치지 않는다.
- **켜고 끄기**: 상태는 레지스트리 자체(앱·설치 파일이 같은 값을 봄). 설치본은 설치 파일이 `win-filetool.exe --ctx-menu=install`로 켜고(사용자가 끈 적이 있으면
  `settings.json`의 `context_menu_off`를 보고 그대로 둠), 지울 때 `--ctx-menu=remove` + NSIS `DeleteRegKey`로 지운다. 휴대용은 기본 꺼짐, [설정]에서 켠다.
  등록된 exe가 지금 exe와 다르면(휴대용을 옮김) [다시 연결]을 보여 준다.

## 0.3.3 가벼운 판 — 문서 도구는 필요할 때만 받기

결정(11-02): 판은 하나(가벼운 판). 앱 exe에서 LibreOffice(zstd 약 105 MB)를 빼고, 문서 → PDF는 이 PC의 한글·Microsoft Office(COM)로 한다.
그것으로 안 될 때만 "문서 도구"(같은 LibreOffice)를 묻고 한 번 받는다.

**길 고르기**(`docs::engine_for`, 순수 함수 `engine_for_ext` + 단위 시험):

| 파일 | 한글·Office 있음 | 없음 |
|---|---|---|
| hwp·hwpx(한글 5·HWPX) | 한글 COM | 안 됨(그림 뽑기만) |
| 한글 97 | 한글 COM | 문서 도구 |
| doc·docx·docm·dot(x)·rtf·odt·txt | Word COM | 문서 도구 |
| xls·xlsx·xlsm·xlsb·xlt(x)·ods·csv | Excel COM | 문서 도구 |
| ppt·pptx·pptm·pps(x)·pot(x)·odp | PowerPoint COM | 문서 도구 |
| Pages·Keynote·Numbers, fodt·fods·fodp·wpd | 문서 도구 | 문서 도구 |

**아직 문서 도구가 필요한 일**(한글·Office가 다 있는 PC에서도): Pages·Keynote·Numbers → PDF / Word·PPT·엑셀 형식([더 보기] Word 형식으로·Office 형식으로),
fodt·wpd 등 → PDF, Word → PPT의 입력이 Pages인 경우. Office가 없는 PC에서는 Word·엑셀·PPT → PDF와 Word → PPT(.doc·.odt·.rtf),
한글이 없는 PC에서는 한글 97 → PDF·PPT. PDF → Word·PPT·TXT, 이미지 뽑기, PDF 편집은 원래 LibreOffice를 쓰지 않는다.
Word → PPT의 .doc·.rtf·.odt는 0.3.3부터 Word가 있으면 Word로 docx를 먼저 만든다.

**Office COM**(`msoffice.rs` + `msoffice.ps1`, 한글 길과 같은 규칙): PowerShell이 Office를 새로 띄움 → 새 프로세스가 없으면(열린 PowerPoint에 붙음)
아무것도 안 하고 그만둠 → **문서를 열기 전에** `AutomationSecurity = 3`(msoAutomationSecurityForceDisable), 알림 끔, Word `DisableAutoMacros`,
Excel `EnableEvents=false`·`AskToUpdateLinks=false`, Word `UpdateLinksAtOpen` 잠시 끔(끝나면 되돌림) → 임시 복사본을 읽기 전용·최근 파일 안 넣음
(Word `AddToRecentFiles:=False`, Excel `AddToMru:=False`, `UpdateLinks:=0`, PowerPoint `WithWindow:=msoFalse`)으로 열기, 틀린 암호를 주어 암호 문서는 바로 실패
→ 임시 폴더에 PDF(`ExportAsFixedFormat` / PowerPoint `SaveAs 32`) → 닫기·`Quit`. 끝나지 않으면 우리가 띄운 프로세스만 강제 종료(180초), 한 번에 하나(잠금 파일).
있는지 보기는 레지스트리만(ProgID → CLSID → LocalServer32, `CurVer`로 판). OOXML 확장자인데 복합 파일이면 암호 문서로 보고 Office를 띄우지 않는다.
PowerPoint가 겹쳐 못 하면 문서 도구가 이미 있을 때만 그것으로 한다.

**문서 도구 묶음**(`doctools.rs`): `win-filetool-doctools-1.zst` = lo-trim의 tar → `zstd -19 --long=27`(0.3.0~0.3.2 앱 안 `lo` 덩어리와 같은 바이트,
lo-trim에서 다시 만들어도 같은 SHA-256). 주소 `…/releases/download/v0.3.3/win-filetool-doctools-1.zst`, SHA-256 `355752ca…`과 크기 105,443,108 B는 exe에 고정.
받기는 WinHTTP(시스템 프록시), 이어받기(`Range`, 서버가 200이면 처음부터), 멈추기·끊김이면 `.part`를 남김, 다 받으면 SHA-256 확인 → 다르면 지우고 거절 →
`tools\doctools\lo.partial`에 풀고 이름 바꾸기. [파일로 넣기]는 고른 파일을 우리 폴더로 복사한 뒤 같은 확인(고른 파일은 지우지 않음).
0.3.0~0.3.2가 풀어 둔 `tools\<판>-<해시>\lo`는 `doctools\lo`로 옮겨 쓴다(새 판의 지난 판 폴더 지우기도 먼저 옮김). 오류 `Error::NeedDocTools` →
화면 `withDocTools()`가 한 번 묻고([받기]/[취소]) 받은 뒤 그 파일을 다시 한다. 같은 작업 안에서 [취소]하면 다시 묻지 않는다.

**시험용 바꾸기**(`testhooks.rs`): `WIN_FILETOOL_HIDE_COM`·`HIDE_HANCOM`·`HIDE_OFFICE`·`DOCTOOLS_URL`·시간 제한 줄이기는 CLI와 `test-hooks` 기능 앱만 읽는다.
배포 앱은 이 값들을 읽지 않는다(0.3.2까지 배포 앱도 읽던 `HIDE_HANCOM` 포함). 해시는 어디서도 바꿀 수 없다.

**네트워크**(다시 점검): ① 새 버전 확인 `GET api.github.com/…/releases/latest`(하루 한 번까지, 끌 수 있음) ② 문서 도구 `GET github.com/…/releases/download/v0.3.3/…`
(사용자가 [받기]를 누를 때만, 이어받기 때 `Range`). 둘 다 User-Agent `win-filetool/<판>`만, 쿠키·식별자 없음. 그 밖에 앱이 인터넷에 연결하는 코드는 없다
(검색 서비스는 이름 있는 파이프, 화면은 로컬 WebView2).
