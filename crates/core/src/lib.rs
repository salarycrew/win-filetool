//! win-filetool 코어. Tauri에 의존하지 않으므로 Linux에서도 `cargo test`로 검증한다.
//!
//! - [`nfc`]: 폴더 안 NFD 파일명을 NFC로 바꾸기 (미리보기 → 적용, 충돌은 건너뛰고 보고)
//! - [`zipnfc`]: 맥에서 만든 zip을 NFC 이름으로 풀기 / NFC 이름으로 다시 압축
//! - [`pdf`]: PDF 합치기·나누기·회전·페이지 삭제·순서 변경 (순수 Rust, lopdf)
//! - [`imageconv`]: 이미지 형식 변환(+HEIC는 ffmpeg로 풀기), [`media`]: 영상·소리(ffmpeg),
//!   [`docs`]: 문서(LibreOffice headless), [`convert`]: 파일 하나 변환 진입점
//! - [`tools`]: ffmpeg/LibreOffice 찾기, [`process`]: argv 실행·시간 제한·취소
//! - [`watch`]: 감시 폴더, [`settings`]: 설정 저장
//! - 1c: [`pdfcompress`]: PDF 압축, [`img2pdf`]: 이미지 → PDF, [`pdfrender`]: PDF → 이미지(PDFium)·이미지 꺼내기,
//!   [`autostart`]: 윈도 시작 시 실행(HKCU Run)
//! - 0.3.0 W2 문서 변환: [`ooxml`]: 공용 DOCX/PPTX 쓰기, [`docx2pptx`]: Word → PPT(제목마다 한 장),
//!   [`pdflayout`]: PDF 글자 배치 분석, [`pdf2docx`]: PDF → Word·TXT, [`pdf2pptx`]: PDF → PPT(쪽 그림/글상자),
//!   [`officeimages`]: 문서·PDF 이미지만 뽑기
//! - 2a: [`search`]: 전체 파일 즉시 검색(색인·스캔·실시간 갱신·질의 언어)
//!
//! 원본은 절대 덮어쓰지 않는다. 출력 경로가 이미 있으면 [`paths::unique_path`]로
//! ` (1)`, ` (2)` …를 붙인 새 이름을 쓴다.

pub mod autostart;
pub mod convert;
pub mod docs;
pub mod docx2pptx;
pub mod error;
pub mod exif;
pub mod heif;
pub mod imageconv;
pub mod img2pdf;
pub mod media;
pub mod nfc;
pub mod officeimages;
pub mod ooxml;
pub mod pages;
pub mod paths;
pub mod pdf;
pub mod pdf2docx;
pub mod pdf2pptx;
pub mod pdfcompress;
pub mod pdflayout;
pub mod pdfrender;
pub mod process;
pub mod samples;
pub mod search;
pub mod settings;
pub mod tempdir;
pub mod tools;
pub mod watch;
pub mod zipnfc;

pub use error::{Error, Result};
