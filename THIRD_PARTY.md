# 함께 든 오픈소스 부품 (요약)

파일 도구의 코드는 MIT([LICENSE](LICENSE))입니다. 배포본(설치 파일·설치 없는 exe)에는 아래 부품이 들어 있습니다.
라이선스 전문과 저작권 표시는 앱의 [앱 정보] → 오픈소스 고지, 또는 배포본과 함께 받는 `THIRD-PARTY-NOTICES.txt`에 있습니다
(빌드 때 `scripts/make-notices.py`가 실제로 링크·포함한 것에서 만듭니다).
GPL·AGPL 부품은 넣지 않았습니다.

| 부품 | 판 | 라이선스 | 어떻게 들어 있나 | 소스 |
|---|---|---|---|---|
| ffmpeg (ffmpeg.exe, ffprobe.exe) | 8.1 | LGPL-2.1-or-later | 별도 실행 파일. `--enable-gpl`·`--enable-nonfree` 없이 최소 빌드 | [소스 저장소](https://github.com/salarycrew/win-filetool-sources) (`ffmpeg-8.1.tar.xz`, configure 줄, `build-ffmpeg.sh`), 원 출처 <https://ffmpeg.org/releases/> |
| LAME | 3.100 | LGPL-2.0-or-later | ffmpeg에 정적 링크(MP3 만들기) | 같은 곳 `lame-3.100.tar.gz`, <https://lame.sourceforge.io/> |
| dav1d | 1.5.1 | BSD-2-Clause | ffmpeg에 정적 링크(AV1 풀기) | 같은 곳 `dav1d-1.5.1.tar.xz`, <https://code.videolan.org/videolan/dav1d> |
| zlib | 1.3.1 | zlib | ffmpeg에 정적 링크 | 같은 곳 `zlib-1.3.1.tar.gz`, <https://zlib.net/> |
| LibreOffice | 26.2.6.3 | MPL-2.0 (+ LibreOffice가 함께 싣는 서드파티 라이선스) | 공식 Windows 판에서 필요 없는 파일만 뺀 별도 프로그램(문서 → PDF). GPL인 poppler PDF 가져오기 부품은 뺌 | 같은 곳 `libreoffice-26.2.6.3.tar.xz`·번역 묶음·`trim-libreoffice.ps1`, 원 출처 <https://download.documentfoundation.org/libreoffice/src/26.2.6/> |
| Microsoft Visual C++ 런타임 | LibreOffice와 함께 | Microsoft 재배포 조건 | LibreOffice 폴더 안 DLL | — |
| PDFium | 151.0.7881.0 (chromium/7881) | BSD-3-Clause (+ FreeType·ICU·libjpeg-turbo·libpng·OpenJPEG·lcms 등, 고지에 전문) | 별도 DLL(PDF 그리기) | <https://pdfium.googlesource.com/pdfium/>, 바이너리 <https://github.com/bblanchon/pdfium-binaries> |
| Rust 라이브러리 (약 310개) | Cargo.lock | 대부분 MIT·Apache-2.0, 일부 BSD·Zlib·Unicode-3.0·MPL-2.0(cssparser, selectors, dtoa-short, option-ext — 고치지 않음) | 앱·명령줄 도구·검색 서비스에 링크 | 각 크레이트의 crates.io 페이지(고지에 저장소 주소) |
| @tauri-apps/api, @tauri-apps/plugin-dialog | 2.12.4, 2.8.4 | MIT OR Apache-2.0 | 화면(웹 UI) 묶음 | <https://github.com/tauri-apps/tauri> |
| Vite 런타임 조각 | 8.3.5 | MIT | 화면 묶음(모듈 미리 불러오기) | <https://github.com/vitejs/vite> |

사용자 PC에 설치된 한컴오피스 한글은 앱에 들어 있지 않고, 있을 때 Windows 자동화(COM)로 부를 뿐입니다.
한글 변환용 보안 모듈 DLL은 한컴 예제를 재배포하지 않고 앱 코드가 만듭니다(`crates/core/src/hwp/hancom.rs`).

## 저장소에만 있는 시험 자료 (배포본에 없음)

| 파일 | 출처 | 라이선스 |
|---|---|---|
| `crates/core/tests/data/030/hwp/hangul97-3.0.hwp` | LibreOffice 소스의 hwpfilter 회귀 시험 자료 | MPL-2.0 (바꾸지 않음) |
| `crates/core/src/samples_heic/*.hevc` | 이 프로젝트가 만든 단색 HEVC 타일(시험용 HEIC 만들기) | MIT(이 저장소) |
