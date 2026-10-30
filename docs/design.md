# win-filetool 설계

상태: 초안. **스택은 사용자 확인 대기(미확정).**

## 1단계

- 파일 변환: 이미지·문서·영상. ffmpeg·LibreOffice 등 기존 도구를 활용한다.
- 감시 폴더 자동 변환.
- 맥↔윈도 호환:
  - HEIC → JPG
  - Pages / Numbers / Keynote 열기·변환
  - 맥에서 만든 zip·파일명의 한글 NFD → NFC 정규화
- PDF: 합치기, 나누기, 회전, 페이지 삭제, 순서 변경, 압축, 이미지↔PDF.
- 본문 텍스트 수정은 범위 밖(사용자 확정).

## 2단계 (1단계 출시 뒤)

- Everything식 전체 파일 즉시 검색. NTFS MFT/USN 방식을 검토한다.

## 스택 (확인 대기)

추천 1안(미확정): Tauri 2 (Rust 코어 + 웹 UI)

- 폴더 감시: `notify` crate
- 정규화: `unicode-normalization`
- 변환: ffmpeg / LibreOffice / libheif 동봉 호출
- PDF: qpdf 또는 lopdf + pdfium
- 2단계 검색: `windows` crate `FSCTL_ENUM_USN_DATA`

대안: .NET 8 + WinUI, Python + Qt.

## 빌드·실행 검증 기기

desktop (Windows, `ssh desktop`, 평소 꺼져 있음).
