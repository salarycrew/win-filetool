; NSIS 설치 파일 훅(Tauri bundler가 넣는다).
; 지울 때 "윈도 시작 시 실행" 값(HKCU Run\win-filetool)도 지워, 지운 앱을 가리키는 시작 항목이 남지 않게 한다.
!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "win-filetool"
!macroend
