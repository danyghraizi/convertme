$ErrorActionPreference = 'Stop'
$installRoot = Join-Path $env:LOCALAPPDATA 'Programs\ConvertMe Image'
$shortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\ConvertMe Image.lnk'
Remove-Item -LiteralPath $shortcutPath -Force -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $installRoot) {
  $resolvedRoot = (Resolve-Path -LiteralPath $installRoot).Path
  $expectedRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs\ConvertMe Image'))
  if ($resolvedRoot -ne $expectedRoot) { throw 'Refusing to remove an unexpected install directory.' }
  Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
}
Write-Output 'ConvertMe Image has been removed. The shared WebView2 Runtime remains installed.'
