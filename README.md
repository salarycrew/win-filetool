# win-filetool

Windows용 파일 도구 앱(창 제목 "파일 도구"). 파일 변환(이미지·문서·영상·PDF)과 맥↔윈도 호환 처리를 한곳에 모으고, 2단계에서 전체 파일 즉시 검색을 더한다.

스택: **Tauri 2** (Rust 코어 + Vite/TypeScript 웹 UI). 설계·단계 구분은 [docs/design.md](docs/design.md).

## 1a에서 되는 것

- 파일·폴더를 창에 끌어다 놓기(또는 고르기) → 작업 선택 → 실행. 저장 위치 기본값은 원본 옆, 원본은 덮어쓰지 않음
- **파일명 한글 깨짐 고치기**: 폴더 안 NFD 이름 → NFC (미리보기 후 적용, 같은 이름이 있으면 건너뛰고 보고)
- **맥에서 만든 zip 풀기**: 이름을 NFC로 고쳐 풀기, `__MACOSX/`·`._*` 제외, NFC 이름으로 다시 압축(선택)
- **PDF**: 합치기(목록 순서), 나누기(한 쪽씩 / `1-3,5` 범위), 회전(90/180/270), 페이지 삭제, 순서 변경

1b(변환·감시 폴더), 1c(PDF 압축, 이미지↔PDF, 설치 파일)는 아직이다.

## 빌드 (K12 Linux → Windows exe)

준비 도구와 자세한 과정은 [docs/design.md](docs/design.md#빌드-경로-k12-크로스-컴파일--desktop-실행).

```sh
npm ci
bash scripts/build-windows.sh
cargo test -p filetool-core
```

결과물:

- `target/x86_64-pc-windows-msvc/release/win-filetool.exe` — 앱(휴대용, 설치 불필요, WebView2 런타임 필요)
- `target/x86_64-pc-windows-msvc/release/filetool-cli.exe` — 같은 코어의 명령줄 도구(`filetool-cli selftest <빈 폴더>`로 점검)

desktop 배치 위치: `C:\Users\user\dev\win-filetool-builds\1a\`.
