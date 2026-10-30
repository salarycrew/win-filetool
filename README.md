# win-filetool

Windows용 파일 도구 앱. 파일 변환(이미지·문서·영상·PDF)과 맥↔윈도 호환 처리를 한곳에 모으고, 2단계에서 전체 파일 즉시 검색을 더한다.

## 범위 요약

- **1단계**: 파일 변환(ffmpeg·LibreOffice 등 기존 도구 활용), 감시 폴더 자동 변환, 맥↔윈도 호환(HEIC→JPG, Pages/Numbers/Keynote, zip·파일명 NFD→NFC), PDF 도구(합치기·나누기·회전·페이지 삭제·순서 변경·압축·이미지↔PDF)
- **2단계**(1단계 출시 뒤): Everything식 전체 파일 즉시 검색
- 범위 밖: 문서 본문 텍스트 수정

기술 스택은 **확인 대기**. 자세한 내용은 [docs/design.md](docs/design.md).
