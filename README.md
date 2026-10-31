# win-filetool

Windows용 파일 도구 앱(창 제목 "파일 도구"). 파일 변환(이미지·문서·영상·PDF)과 맥↔윈도 호환 처리를 한곳에 모으고, 2단계에서 전체 파일 즉시 검색을 더한다.

스택: **Tauri 2** (Rust 코어 + Vite/TypeScript 웹 UI). 설계·단계 구분은 [docs/design.md](docs/design.md).

## 1a에서 되는 것

- 파일·폴더를 창에 끌어다 놓기(또는 고르기) → 작업 선택 → 실행. 저장 위치 기본값은 원본 옆, 원본은 덮어쓰지 않음
- **파일명 한글 깨짐 고치기**: 폴더 안 NFD 이름 → NFC (미리보기 후 적용, 같은 이름이 있으면 건너뛰고 보고)
- **맥에서 만든 zip 풀기**: 이름을 NFC로 고쳐 풀기, `__MACOSX/`·`._*` 제외, NFC 이름으로 다시 압축(선택)
- **PDF**: 합치기(목록 순서), 나누기(한 쪽씩 / `1-3,5` 범위), 회전(90/180/270), 페이지 삭제, 순서 변경

## 1b에서 되는 것

- **이미지**: JPG·PNG·WebP·BMP·TIFF·GIF(첫 장면) 서로 바꾸기, 품질(JPG·WebP), 긴 변 줄이기, 사진 방향 바로잡기, 촬영 날짜(EXIF) 유지(JPG·PNG)
- **HEIC(아이폰 사진) → JPG/PNG**: ffmpeg로 풀고, 512px 타일 grid·회전도 처리
- **영상·소리**(ffmpeg): MOV/MKV/AVI/WebM → MP4, 메신저용으로 줄이기(크기·해상도), 소리만 MP3/M4A, 짧은 GIF. 진행률·취소
- **문서**(LibreOffice): Word/Excel/PowerPoint/ODF/Pages/Numbers/Keynote → PDF, Pages→DOCX, Numbers→XLSX, Keynote→PPTX
- **감시 폴더**: 폴더에 들어온 파일을 자동 변환해 하위 폴더(기본 `변환됨`)에 저장(앱이 켜져 있는 동안). 복사 중인 파일은 기다리고, 한 번 처리한 파일은 다시 처리하지 않음
- **도구 설정**: ffmpeg·LibreOffice를 찾아 버전 표시, 없으면 받는 곳 안내, 직접 경로 지정

ffmpeg와 LibreOffice는 앱에 들어 있지 않다. 따로 설치하거나(관리자 권한이 없으면 LibreOffice 설치 파일을 폴더에 풀기:
`msiexec /a LibreOffice_x.y.z_Win_x86-64.msi /qn TARGETDIR=%USERPROFILE%\LibreOffice`) [도구 설정]에서 위치를 고른다.

## 1c에서 되는 것

- **PDF 용량 줄이기**: 화면용 / 보통 / 인쇄용. 들어 있는 사진만 해상도·품질을 낮추고 글자·선·표는 그대로. 파일마다 전후 크기 표시,
  줄지 않으면 새 파일을 만들지 않고 알림. 암호가 걸린 PDF는 안내 후 건너뜀
- **이미지 → PDF**: 여러 장을 목록 순서대로 한 PDF로(JPG·PNG·WebP·BMP·TIFF·HEIC). 사진 방향(EXIF) 반영, JPG는 다시 압축하지 않음.
  쪽 크기: 이미지 크기 그대로 / A4 세로·가로에 맞춤, 여백
- **PDF → 이미지**: 쪽마다 PNG/JPG(72·150·300 DPI, 쪽 범위), 또는 들어 있는 사진만 꺼내기
- **설치 파일·아이콘**: 사용자별 설치(관리자 권한 없음), 시작 메뉴 "파일 도구", 제거 프로그램. 휴대용 묶음도 계속 제공
- **윈도우 시작 시 실행**([설정], 기본 꺼짐): 로그인하면 창 없이 알림 영역(트레이)에서 시작해 감시 폴더를 돌림.
  감시 규칙이 켜져 있으면 창을 닫아도 트레이에 남음(트레이 메뉴: 열기 / 감시 일시정지 / 끝내기). 앱은 한 번만 실행됨(두 번째 실행은 기존 창을 앞으로)

## 설치 / 제거

- **설치 파일**: `파일도구_0.1.0_x64-setup.exe` 실행 → 관리자 권한 없이 `%LOCALAPPDATA%\파일 도구\`에 설치, 시작 메뉴에 "파일 도구".
  조용히 설치: `파일도구_0.1.0_x64-setup.exe /S`
- **제거**: [설정 > 앱 > 설치된 앱]에서 "파일 도구" 제거, 또는 `%LOCALAPPDATA%\파일 도구\uninstall.exe` (조용히: `/S`).
  앱 파일·시작 메뉴·"윈도우 시작 시 실행" 값을 지운다. 설정 폴더는 제거 화면에서 "앱 데이터 삭제"를 고를 때만 지운다
- **휴대용**: `win-filetool-0.1.0-portable.zip`을 풀고 `win-filetool.exe` 실행(`pdfium.dll`이 같은 폴더에 있어야 PDF→이미지가 됨)
- 서명하지 않은 프로그램이라 처음 실행할 때 SmartScreen("Windows의 PC 보호")이 뜰 수 있다 → [추가 정보] → [실행]

### 설정이 있는 곳

- 설정(도구 경로·감시 규칙): `%APPDATA%\kr.salarycrew.winfiletool\settings.json` (감시 기록 `watch-state.json`도 같은 폴더)
- 윈도우 시작 시 실행: 레지스트리 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `win-filetool`
- 작업 중 임시 파일: `%TEMP%\win-filetool\` (끝나면 지움)

## 빌드 (K12 Linux → Windows exe)

준비 도구와 자세한 과정은 [docs/design.md](docs/design.md#빌드-경로-k12-크로스-컴파일--desktop-실행).

```sh
npm ci
bash scripts/fetch-pdfium.sh      # PDF→이미지 렌더러(PDFium) — third_party/에 받음(저장소에 넣지 않음)
bash scripts/fetch-nsis.sh        # 설치 파일용 makensis(sudo 없이 ~/.local/opt에)
bash scripts/build-windows.sh
cargo test -p filetool-core
```

결과물(`dist-win/`):

- `파일도구_0.1.0_x64-setup.exe` — 사용자별 설치 파일(NSIS, 한국어)
- `win-filetool-0.1.0-portable\` (+ `.zip`) — `win-filetool.exe`(앱, WebView2 런타임 필요), `pdfium.dll`, `licenses\`,
  `filetool-cli.exe`(같은 코어의 명령줄 도구: `selftest <빈 폴더>`로 점검, `tools`, `image`/`media`/`doc`/`watch`,
  1c `pdf-compress`/`img2pdf`/`pdf2img`/`pdf-images`/`autostart`)

desktop 배치 위치: `C:\Users\user\dev\win-filetool-builds\<단계>\` (1a, 1b, 1c).
