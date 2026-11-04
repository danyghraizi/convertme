; The standalone installer is supplied and verified by the bundle driver.
!ifndef CONVERTME_RUNTIME_EMBEDDED
!define CONVERTME_RUNTIME_EMBEDDED
!if "$%CONVERTME_WEBVIEW2_INSTALLER%" == ""
!error "CONVERTME_WEBVIEW2_INSTALLER must identify the verified standalone installer"
!endif
!endif

!macro NSIS_HOOK_PREINSTALL
  InitPluginsDir
  File /oname=$PLUGINSDIR\convertme-webview2-runtime.exe "$%CONVERTME_WEBVIEW2_INSTALLER%"
  Push $0
  Push $1
  SetRegView 32
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" runtime_check_user
  StrCmp $0 "0.0.0.0" runtime_check_user runtime_ready
  runtime_check_user:
  ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "0.0.0.0" 0 +2
  StrCpy $0 ""
  StrCmp $0 "" 0 runtime_ready
  ExecWait '"$PLUGINSDIR\convertme-webview2-runtime.exe" /silent /install' $1
  StrCmp $1 "0" 0 runtime_failed
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" runtime_recheck_user
  StrCmp $0 "0.0.0.0" runtime_recheck_user runtime_ready
  runtime_recheck_user:
  ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" runtime_failed
  StrCmp $0 "0.0.0.0" runtime_failed
  runtime_ready:
  Pop $1
  Pop $0
  Goto runtime_done
  runtime_failed:
  MessageBox MB_ICONSTOP "WebView2 Runtime installation failed or did not register. ConvertMe was not installed."
  Abort
  runtime_done:
!macroend

; Remove only the autostart command written by this installation. Settings,
; history, and the shared WebView2 runtime deliberately remain untouched.
!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "ConvertMe"
  StrCmp $0 '$"$INSTDIR\convertme.exe$" --background' 0 +2
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "ConvertMe"
  Pop $0
!macroend
