; NSIS 설치 파일 훅(Tauri bundler가 넣는다).
; 지울 때 "윈도 시작 시 실행" 값(HKCU Run\win-filetool)도 지워, 지운 앱을 가리키는 시작 항목이 남지 않게 한다.
; 2c: 빠른 검색 서비스(FileToolSvc, 선택 설치)가 있으면 함께 지울지 묻는다. 서비스는 관리자 권한으로 등록돼 있어
;     지우려면 UAC 창이 한 번 뜬다([예] → 앱 폴더의 filetool-svc.exe uninstall을 관리자 권한으로).
;     [아니요]·조용히 제거(/S)일 때는 서비스가 남는다(서비스 실행 파일은 Program Files에 따로 있어 계속 동작) —
;     나중에 관리자 명령 프롬프트에서 "C:\Program Files\파일 도구 서비스\filetool-svc.exe" uninstall 로 지운다.
; 0.3.0: 한글 변환용 보안 모듈 — 앱이 만든 DLL 폴더(%LOCALAPPDATA%\kr.salarycrew.winfiletool\hwp)와
;        남아 있을 수 있는 HKCU 등록값(변환 중에만 두지만, 변환 중 강제 종료됐을 때 대비)을 지운다.
; 0.3.2: 탐색기 우클릭 메뉴("파일도구"). 설치하면 켠다(사용자가 [설정]에서 끈 적이 있으면 앱이 그대로 둠),
;        지우면 HKCU\Software\Classes 아래 우리 키만 지운다(앱이 지우고, 앱을 못 부를 때를 대비해 여기서도 지움).
!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\win-filetool.exe" --ctx-menu=install'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\win-filetool.exe" --ctx-menu=remove'
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pdf\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.png\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.heic\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.heif\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.webp\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.gif\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tif\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tiff\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.hwp\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.hwpx\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.doc\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.docx\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.rtf\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.odt\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.xls\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.xlsx\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.ods\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.ppt\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.pptx\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.odp\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.zip\shell\WinFileTool"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\WinFileTool"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "win-filetool"
  DeleteRegValue HKCU "Software\HNC\HwpAutomation\Modules" "WinFileToolPathCheck"
  RMDir /r "$LOCALAPPDATA\kr.salarycrew.winfiletool\hwp"
  RMDir /r "$LOCALAPPDATA\kr.salarycrew.winfiletool\office"
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\FileToolSvc" "ImagePath"
  StrCmp $0 "" wft_svc_done
  IfSilent wft_svc_done
  IfFileExists "$INSTDIR\filetool-svc.exe" 0 wft_svc_done
  MessageBox MB_YESNO|MB_ICONQUESTION "빠른 검색 서비스(FileToolSvc)가 설치되어 있습니다.$\r$\n함께 제거할까요? 관리자 권한 확인 창이 뜹니다.$\r$\n$\r$\n[아니요]를 누르면 서비스는 그대로 남습니다." IDNO wft_svc_done
  ExecShellWait "runas" "$INSTDIR\filetool-svc.exe" "uninstall" SW_HIDE
  wft_svc_done:
!macroend
