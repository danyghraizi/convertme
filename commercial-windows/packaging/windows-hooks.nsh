; Install the verified offline WebView2 runtime before installing the app.
!macro NSIS_HOOK_PREINSTALL
  InitPluginsDir
  File /oname=$PLUGINSDIR\convertme-webview2.exe "$%CONVERTME_WEBVIEW2_INSTALLER%"
  ExecWait '"$PLUGINSDIR\convertme-webview2.exe" /silent /install' $0
  StrCmp $0 "0" runtime_installed
  MessageBox MB_ICONSTOP "WebView2 could not be installed (exit code $0). ConvertMe Image was not installed."
  Abort
  runtime_installed:
!macroend
