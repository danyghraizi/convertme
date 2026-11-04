param(
  [string]$CertificateThumbprint,
  [switch]$RequireSigning
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$appRoot = Join-Path $repoRoot 'commercial-windows'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$rustup = Join-Path (Split-Path $cargo) 'rustup.exe'
$installedToolchains = & $rustup toolchain list
$lockedToolchain = if ($installedToolchains -match '^1\.99\.1(?:-|\s|$)') { '1.99.1' } else { 'stable' }
$env:RUSTUP_TOOLCHAIN = $lockedToolchain
$rustVersion = (& (Join-Path (Split-Path $cargo) 'rustc.exe') --version | Out-String).Trim()
if ($rustVersion -notmatch '^rustc 1\.99\.1(?:\s|$)') { throw "Expected Rust 1.99.1, found $rustVersion." }
$target = 'x86_64-pc-windows-msvc'
$runtimeLockPath = Join-Path $repoRoot 'packaging\windows\webview2.lock.json'
$runtimeLock = Get-Content -Raw $runtimeLockPath | ConvertFrom-Json
$runtimeEntry = $runtimeLock.architectures.x64
$runtime = Join-Path $repoRoot "vendor\windows\runtime\$($runtimeEntry.file)"

if (-not (Test-Path -LiteralPath $cargo)) { throw "Cargo was not found at $cargo." }
if (-not (Test-Path -LiteralPath $runtime)) {
  New-Item -ItemType Directory -Force -Path (Split-Path $runtime) | Out-Null
  & (Join-Path $PSScriptRoot 'prepare-windows-runtime.ps1') -Target $target -Out $runtime
  if ($LASTEXITCODE -ne 0) { throw 'Could not fetch and verify the locked Microsoft WebView2 runtime.' }
}
$runtimeHash = (Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash.ToLowerInvariant()
if ($runtimeHash -ne $runtimeEntry.sha256) { throw 'The WebView2 runtime SHA-256 does not match its lock record.' }
$runtimeSignature = Get-AuthenticodeSignature -LiteralPath $runtime
if ($runtimeSignature.Status -ne 'Valid' -or $runtimeSignature.SignerCertificate.Thumbprint -ne $runtimeEntry.signer_thumbprint) {
  throw 'The WebView2 runtime signature or Microsoft signer thumbprint does not match its lock record.'
}

& $cargo fmt --manifest-path (Join-Path $appRoot 'Cargo.toml') --check
if ($LASTEXITCODE -ne 0) { throw 'Rust formatting check failed.' }
& $cargo test --manifest-path (Join-Path $appRoot 'Cargo.toml') --locked
if ($LASTEXITCODE -ne 0) { throw 'Commercial image conversion tests failed.' }
& (Join-Path $PSScriptRoot 'gen-commercial-notices.ps1') -Cargo $cargo
if ($LASTEXITCODE -ne 0) { throw 'Third-party license notice generation failed.' }
& $cargo build --manifest-path (Join-Path $appRoot 'Cargo.toml') --locked --release --target $target
if ($LASTEXITCODE -ne 0) { throw 'Windows x64 release build failed.' }

$appExe = Join-Path $appRoot "target\$target\release\convertme-commercial.exe"
if (-not (Test-Path -LiteralPath $appExe)) { throw 'The release executable was not produced.' }
$signed = $false
if ($CertificateThumbprint) {
  $signTool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } | Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $signTool) { throw 'Windows SDK signtool.exe was not found.' }
  $certificate = Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
    Where-Object Thumbprint -EQ ($CertificateThumbprint -replace '\s', '') | Select-Object -First 1
  if (-not $certificate) { throw 'The requested signing certificate is not installed in a personal certificate store.' }
  & $signTool.FullName sign /fd SHA256 /td SHA256 /tr 'http://timestamp.digicert.com' /sha1 $certificate.Thumbprint $appExe
  if ($LASTEXITCODE -ne 0) { throw 'Authenticode signing failed.' }
  $appSignature = Get-AuthenticodeSignature -LiteralPath $appExe
  if ($appSignature.Status -ne 'Valid' -or $appSignature.SignerCertificate.Thumbprint -ne $certificate.Thumbprint) {
    throw 'Signed executable verification failed.'
  }
  $signed = $true
}
elseif ($RequireSigning) { throw 'A signing certificate thumbprint is required for a signed candidate.' }

$manifest = Get-Content -Raw (Join-Path $appRoot 'Cargo.toml') | Select-String -Pattern '(?m)^version\s*=\s*"([^"]+)"' | ForEach-Object { $_.Matches[0].Groups[1].Value } | Select-Object -First 1
if (-not $manifest) { throw 'Could not read the application version from Cargo.toml.' }
$edition = if ($signed) { 'candidate-signed' } else { 'candidate-unsigned' }
$artifactRoot = Join-Path $repoRoot 'artifacts\commercial-windows'
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$stage = Join-Path $artifactRoot ("stage-" + [Guid]::NewGuid().ToString('N'))
$portable = Join-Path $stage 'portable'
$source = Join-Path $stage 'third-party-source'
New-Item -ItemType Directory -Force -Path $portable, $source | Out-Null

Copy-Item $appExe (Join-Path $portable 'convertme-commercial.exe')
Copy-Item $runtime (Join-Path $portable $runtimeEntry.file)
Copy-Item (Join-Path $appRoot 'LICENSE') $portable
Copy-Item (Join-Path $appRoot 'THIRD_PARTY_NOTICES.md') $portable
Copy-Item (Join-Path $appRoot 'packaging\THIRD_PARTY_SOURCE_NOTICES.md') $portable
Copy-Item (Join-Path $appRoot 'README.md') $portable
Copy-Item (Join-Path $appRoot 'packaging\Install-ConvertMeImage.ps1') $portable
Copy-Item (Join-Path $appRoot 'packaging\Uninstall-ConvertMeImage.ps1') $portable
Copy-Item (Join-Path $appRoot 'packaging\SOURCE-CODE.txt') $portable
Copy-Item (Join-Path $appRoot 'packaging\THIRD_PARTY_SOURCE_NOTICES.md') $portable
if ($signed) {
  foreach ($scriptName in @('Install-ConvertMeImage.ps1', 'Uninstall-ConvertMeImage.ps1')) {
    $scriptPath = Join-Path $portable $scriptName
    $scriptSignature = Set-AuthenticodeSignature -FilePath $scriptPath -Certificate $certificate -TimestampServer 'http://timestamp.digicert.com' -HashAlgorithm SHA256
    if ($scriptSignature.Status -ne 'Valid') { throw "PowerShell script signing failed: $scriptName ($($scriptSignature.Status))" }
  }
}

$sourceLockDirectory = Join-Path $source 'source-metadata'
New-Item -ItemType Directory -Force -Path $sourceLockDirectory | Out-Null
Copy-Item (Join-Path $appRoot 'Cargo.lock') (Join-Path $sourceLockDirectory 'Cargo.lock')
Copy-Item (Join-Path $appRoot 'THIRD_PARTY_NOTICES.md') $source
Copy-Item (Join-Path $appRoot 'packaging\THIRD_PARTY_SOURCE_NOTICES.md') $source
$mplSource = Join-Path $source 'third-party-source\mpl-2.0'
New-Item -ItemType Directory -Force -Path $mplSource | Out-Null
$registryRoots = Get-ChildItem (Join-Path $env:USERPROFILE '.cargo\registry\src') -Directory -ErrorAction SilentlyContinue
foreach ($crate in @('dtoa-short-0.3.5', 'cssparser-macros-0.7.1', 'cssparser-0.37.0', 'option-ext-0.2.0', 'selectors-0.38.0')) {
  $crateSource = $registryRoots | ForEach-Object { Join-Path $_.FullName $crate } | Where-Object { Test-Path -LiteralPath $_ -PathType Container } | Select-Object -First 1
  if (-not $crateSource) { throw "Required MPL-2.0 source is missing from Cargo's registry cache: $crate" }
  Copy-Item $crateSource $mplSource -Recurse
}
$sourceSummary = @"
ConvertMe Image $manifest Windows x64 third-party source and notice package.

The package provides source and license materials for dependencies distributed with the matching candidate. ConvertMe's first-party application source is proprietary and is not included. The exact Cargo.lock is retained under source-metadata/ for dependency version mapping.
"@
Set-Content -LiteralPath (Join-Path $source 'README.txt') -Value $sourceSummary -Encoding utf8

$runtimeRecord = [ordered]@{
  name = $runtimeEntry.file
  sha256 = $runtimeHash
  version = $runtimeEntry.file_version
  signer = $runtimeSignature.SignerCertificate.Subject
  signer_thumbprint = $runtimeSignature.SignerCertificate.Thumbprint
}
$readmeExtra = @"

Windows x64 package
- WebView2 Evergreen Runtime installer: $($runtimeEntry.file_version), Microsoft signed, SHA-256 $runtimeHash.
- Run Install-ConvertMeImage.ps1 to install the runtime and create a current-user Start Menu shortcut.
- Signing status: $(if ($signed) { 'the executable and install/uninstall scripts are Authenticode signed; verify each signature' } else { 'unsigned internal candidate; not ready for public release' }).
- Runtime detail: $($runtimeRecord | ConvertTo-Json -Compress)
"@
Add-Content -LiteralPath (Join-Path $portable 'README.md') -Value $readmeExtra -Encoding utf8
$sourcePointer = Join-Path $portable 'SOURCE-CODE.txt'
Set-Content -LiteralPath $sourcePointer -Value @"
The matching third-party source and notices package distributed with this candidate is:
ConvertMeImage-$manifest-win-x64-third-party-source.zip
It contains the dependency lockfile, generated third-party notices, and the MPL-2.0 dependency sources. ConvertMe's first-party application source is proprietary and is not included.
"@ -Encoding utf8

foreach ($folder in @($portable, $source)) {
  $lines = Get-ChildItem -LiteralPath $folder -File -Recurse | Sort-Object FullName | ForEach-Object {
    $relative = $_.FullName.Substring($folder.Length + 1).Replace('\', '/')
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $relative"
  }
  Set-Content -LiteralPath (Join-Path $folder 'SHA256SUMS.txt') -Value $lines -Encoding ascii
}
$zipTimestamp = [DateTime]::new(2000, 1, 1, 0, 0, 0, [DateTimeKind]::Local)
foreach ($folder in @($portable, $source)) {
  (Get-Item -LiteralPath $folder).LastWriteTime = $zipTimestamp
  Get-ChildItem -LiteralPath $folder -Force -Recurse | ForEach-Object { $_.LastWriteTime = $zipTimestamp }
}

$portableZip = Join-Path $artifactRoot "ConvertMeImage-$manifest-win-x64-$edition-portable.zip"
$sourceZip = Join-Path $artifactRoot "ConvertMeImage-$manifest-win-x64-third-party-source.zip"
Remove-Item -LiteralPath $portableZip, $sourceZip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $portable '*') -DestinationPath $portableZip -CompressionLevel Optimal
Compress-Archive -Path (Join-Path $source '*') -DestinationPath $sourceZip -CompressionLevel Optimal
Write-Output "Portable candidate: $portableZip"
Write-Output "Matching source archive: $sourceZip"
$resolvedStage = (Resolve-Path -LiteralPath $stage).Path
$resolvedArtifactRoot = [IO.Path]::GetFullPath($artifactRoot).TrimEnd('\') + '\'
if (-not $resolvedStage.StartsWith($resolvedArtifactRoot, [StringComparison]::OrdinalIgnoreCase) -or
    (Split-Path -Leaf $resolvedStage) -notmatch '^stage-[0-9a-f]{32}$') {
  throw 'Refusing to remove an unexpected staging path.'
}
Remove-Item -LiteralPath $resolvedStage -Recurse -Force
