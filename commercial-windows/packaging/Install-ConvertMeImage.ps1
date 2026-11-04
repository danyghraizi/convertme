$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$runtime = Join-Path $here 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
$installRoot = Join-Path $env:LOCALAPPDATA 'Programs\ConvertMe Image'
$manifest = Join-Path $here 'SHA256SUMS.txt'
foreach ($line in Get-Content -LiteralPath $manifest) {
  $parts = $line -split '  ', 2
  if ($parts.Count -ne 2) { throw 'The package SHA-256 manifest is malformed.' }
  $file = Join-Path $here $parts[1].Replace('/', [IO.Path]::DirectorySeparatorChar)
  if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Package file is missing: $($parts[1])" }
  $actual = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actual -ne $parts[0]) { throw "Package file failed SHA-256 verification: $($parts[1])" }
}

function Test-WebView2RuntimeInstalled {
  $clientId = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  $keys = @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$clientId",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$clientId",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$clientId"
  )
  foreach ($key in $keys) {
    $property = Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue
    $version = $null
    if ($property -and [version]::TryParse($property.pv, [ref]$version) -and $version -gt [version]'0.0') {
      return $true
    }
  }
  return $false
}

$runtimeProcess = $null
if (-not (Test-WebView2RuntimeInstalled)) {
  $runtimeProcess = Start-Process -FilePath $runtime -ArgumentList '/silent', '/install' -Wait -PassThru
  if ($runtimeProcess.ExitCode -notin @(0, 3010)) {
    throw "WebView2 Runtime installation failed with exit code $($runtimeProcess.ExitCode)."
  }
}

New-Item -ItemType Directory -Force -Path $installRoot | Out-Null
Copy-Item (Join-Path $here 'convertme-commercial.exe') $installRoot -Force
Copy-Item (Join-Path $here 'LICENSE') $installRoot -Force
Copy-Item (Join-Path $here 'THIRD_PARTY_NOTICES.md') $installRoot -Force
Copy-Item (Join-Path $here 'Uninstall-ConvertMeImage.ps1') $installRoot -Force

$shortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\ConvertMe Image.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = Join-Path $installRoot 'convertme-commercial.exe'
$shortcut.WorkingDirectory = $installRoot
$shortcut.Description = 'Convert images locally with ConvertMe Image'
$shortcut.Save()
Write-Output "Installed ConvertMe Image to $installRoot"
if ($runtimeProcess -and $runtimeProcess.ExitCode -eq 3010) { Write-Warning 'Windows requested a restart after the WebView2 Runtime update.' }
