# Build unsigned NSIS and portable artifacts from a complete Windows payload.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string] $Target,
    [Parameter(Mandatory = $true)]
    [ValidateSet('gpl', 'commercial')]
    [string] $Edition,
    [string] $Python = 'python',
    [Parameter(Mandatory = $true)]
    [string] $HelperPayload,
    [string] $RuntimeInstaller,
    [string] $ComplianceDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
$root = Split-Path -Parent $PSScriptRoot
$Edition = $Edition.ToLowerInvariant()
if ($Edition -eq 'commercial') {
    throw 'The commercial edition is not configured: the current app and Windows helper closure are GPL/AGPL. Keep using the GPL edition until an approved proprietary dependency and feature manifest exists.'
}
$arch = if ($Target -eq 'x86_64-pc-windows-msvc') { 'x64' } else { 'arm64' }
$runtimeArch = if ($arch -eq 'x64') { 'X64' } else { 'ARM64' }
$tools = Join-Path $root "vendor/windows/$Target/tools"
$out = Join-Path $root "artifacts/windows/$Target"

function Invoke-Native([string] $Program, [string[]] $Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
function Require-File([string] $Path, [string] $Description) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Description is missing: $Path" }
    if ((Get-Item -LiteralPath $Path).Length -eq 0) { throw "$Description is empty: $Path" }
}
function Get-Machine([string] $Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = New-Object IO.BinaryReader($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) { throw "Not a PE executable: $Path" }
        $stream.Position = 0x3c
        $offset = $reader.ReadUInt32()
        if ($offset -gt $stream.Length - 6) { throw "Invalid PE header: $Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550) { throw "Invalid PE signature: $Path" }
        switch ($reader.ReadUInt16()) {
            0x8664 { return 'AMD64' }
            0xaa64 { return 'ARM64' }
            0x014c { return 'I386' }
            default { throw "Unknown PE machine in $Path" }
        }
    } finally { $reader.Dispose() }
}
function Get-Sha256([string] $Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Resolve-ContainedFile([string] $Base, [string] $Relative) {
    if ([IO.Path]::IsPathRooted($Relative)) { throw "Absolute manifest path: $Relative" }
    $basePath = [IO.Path]::GetFullPath($Base).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $full = [IO.Path]::GetFullPath((Join-Path $Base $Relative))
    if (-not $full.StartsWith($basePath, [StringComparison]::OrdinalIgnoreCase)) { throw "Manifest path escapes its root: $Relative" }
    Require-File $full 'Manifest file'
    return $full
}

foreach ($name in @('TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD', 'WINDOWS_CERTIFICATE', 'WINDOWS_CERTIFICATE_PASSWORD')) {
    if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) { throw "Unsigned build requires $name to be unset" }
}
if (-not $RuntimeInstaller) { throw 'Supply -RuntimeInstaller with the matching standalone offline WebView2 installer.' }
if (-not $ComplianceDir) { throw 'Supply -ComplianceDir with verified third-party licenses, Rust license inventory, and corresponding source archives.' }
$RuntimeInstaller = (Resolve-Path -LiteralPath $RuntimeInstaller).Path
$ComplianceDir = (Resolve-Path -LiteralPath $ComplianceDir).Path
Require-File $RuntimeInstaller 'Offline WebView2 installer'
$runtimeName = "MicrosoftEdgeWebView2RuntimeInstaller$runtimeArch.exe"
if ([IO.Path]::GetFileName($RuntimeInstaller) -ine $runtimeName) { throw "Expected the $runtimeName offline installer" }
$signature = Get-AuthenticodeSignature -LiteralPath $RuntimeInstaller
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
    throw 'The offline WebView2 installer must have a valid Microsoft Authenticode signature.'
}
& (Join-Path $PSScriptRoot 'prepare-windows-runtime.ps1') -Target $Target -Out (Split-Path $RuntimeInstaller) -VerifyInstaller $RuntimeInstaller
Require-File (Join-Path $ComplianceDir 'THIRD-PARTY-RUST.md') 'Rust dependency license inventory'
# Compliance materials are supplied with a hash inventory so missing or changed
# source and license files cannot silently produce an incomplete distribution.
$complianceManifest = Join-Path $ComplianceDir 'manifest.json'
Require-File $complianceManifest 'Compliance manifest'
$compliance = Get-Content -LiteralPath $complianceManifest -Raw | ConvertFrom-Json
if ($compliance.schema_version -ne 1 -or -not $compliance.files) { throw 'Invalid compliance manifest' }
$requiredSources = @('ffmpeg', 'ghostscript', 'poppler', 'pandoc', 'pymupdf', 'convertme-pdf')
foreach ($component in $requiredSources) {
    if (-not @($compliance.files | Where-Object { $_.component -eq $component -and $_.kind -eq 'source' }).Count) { throw "Compliance manifest lacks corresponding source for $component" }
}
if (-not @($compliance.files | Where-Object { $_.kind -eq 'license' }).Count) { throw 'Compliance manifest lacks third-party license texts' }
$materialPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
foreach ($item in $compliance.files) {
    if (-not $materialPaths.Add($item.path.Replace('\', '/'))) { throw "Duplicate compliance path: $($item.path)" }
    $file = Resolve-ContainedFile $ComplianceDir $item.path
    if ((Get-Sha256 $file) -ne $item.sha256) { throw "Compliance hash mismatch: $($item.path)" }
}
Get-ChildItem -LiteralPath $ComplianceDir -Recurse -Force | ForEach-Object {
    if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Compliance materials contain a reparse point: $($_.FullName)" }
    if (-not $_.PSIsContainer) {
        $relative = $_.FullName.Substring($ComplianceDir.TrimEnd('\', '/').Length + 1).Replace('\', '/')
        if ($relative -ne 'manifest.json' -and -not $materialPaths.Contains($relative)) { throw "Unowned compliance file: $relative" }
    }
}
if ($compliance.target -ne $Target) { throw 'Compliance materials target mismatch' }

$HelperPayload = (Resolve-Path -LiteralPath $HelperPayload).Path.TrimEnd('\', '/')
Invoke-Native $Python @((Join-Path $PSScriptRoot 'vendor-windows.py'), '--target', $Target, '--validate-payload', $HelperPayload)
if ((Get-Sha256 (Join-Path $HelperPayload 'manifest.json')) -ne $compliance.payload_manifest_sha256) { throw 'Compliance materials are not bound to this payload manifest' }
if ((Get-Sha256 (Join-Path $root 'packaging/sources.lock.json')) -ne $compliance.sources_lock_sha256) { throw 'Compliance materials are not bound to the current sources lock' }
$tools = [IO.Path]::GetFullPath($tools).TrimEnd('\', '/')
if (-not $HelperPayload.Equals($tools, [StringComparison]::OrdinalIgnoreCase)) {
    $parent = Split-Path $tools
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    $staged = Join-Path $parent ('.tools-' + [guid]::NewGuid().ToString('N'))
    $backup = Join-Path $parent ('.tools-backup-' + [guid]::NewGuid().ToString('N'))
    try {
        Copy-Item -LiteralPath $HelperPayload -Destination $staged -Recurse
        Invoke-Native $Python @((Join-Path $PSScriptRoot 'vendor-windows.py'), '--target', $Target, '--validate-payload', $staged)
        if ((Get-Sha256 (Join-Path $staged 'manifest.json')) -ne $compliance.payload_manifest_sha256) { throw 'Staged payload differs from compliance binding' }
        if (Test-Path -LiteralPath $tools) { Move-Item -LiteralPath $tools -Destination $backup }
        try { Move-Item -LiteralPath $staged -Destination $tools } catch {
            if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $tools }
            throw
        }
        if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Recurse -Force }
    } finally { if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Recurse -Force } }
}
$previousRuntime = [Environment]::GetEnvironmentVariable('CONVERTME_WEBVIEW2_INSTALLER')
$env:CONVERTME_WEBVIEW2_INSTALLER = $RuntimeInstaller
$previousComplianceDir = [Environment]::GetEnvironmentVariable('CONVERTME_COMPLIANCE_DIR')
$env:CONVERTME_COMPLIANCE_DIR = $ComplianceDir
Push-Location $root
try {
    Invoke-Native $Python @('scripts/debrand.py', '--src')
    Invoke-Native $Python @('scripts/vendor-windows.py', '--target', $Target, '--validate-payload', $tools)
    $manifestPath = Join-Path $tools 'manifest.json'
    Require-File $manifestPath 'Windows payload manifest'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.schema_version -ne 1 -or $manifest.helper_arch -ne 'x86_64' -or -not $manifest.executables) { throw 'Invalid Windows payload manifest' }
    $required = @('ffmpeg', 'ffprobe', 'ffplay', 'magick', 'gs', 'qpdf', 'pdftoppm', 'pdftotext', 'pandoc', '7zz', 'tesseract', 'convertme-pdf', 'pdfinfo')
    $helpers = @{}
    foreach ($key in $required) {
        $entry = $manifest.executables.PSObject.Properties[$key]
        if (-not $entry -or -not $entry.Value -or $entry.Value -isnot [string]) { throw "Missing payload executable: $key" }
        $file = Resolve-ContainedFile $tools $entry.Value
        $machine = Get-Machine $file
        if ($machine -ne 'AMD64') { throw "Expected AMD64 helper for ${key}: $file ($machine)" }
        $helpers[$key] = @{ path = $entry.Value; sha256 = (Get-Sha256 $file); machine = $machine }
        $versionArg = switch ($key) {
            { $_ -in @('ffmpeg', 'ffprobe', 'ffplay', 'magick') } { '-version'; break }
            { $_ -in @('pdftoppm', 'pdftotext', 'pdfinfo') } { '-v'; break }
            '7zz' { 'i'; break }
            default { '--version' }
        }
        # Several upstream tools print their version on stderr.
        $oldPreference = $ErrorActionPreference
        try {
            $ErrorActionPreference = 'Continue'
            $versionOutput = & $file $versionArg 2>&1 | Out-String
            $versionExit = $LASTEXITCODE
        } finally { $ErrorActionPreference = $oldPreference }
        if ($versionExit -ne 0 -or [string]::IsNullOrWhiteSpace($versionOutput)) { throw "Cannot read version of helper $key" }
        $helpers[$key].version = $versionOutput.Trim()
    }
    $eng = Get-ChildItem -LiteralPath $tools -Filter 'eng.traineddata' -Recurse -File
    if (@($eng).Count -ne 1 -or $eng.Length -eq 0) { throw 'Payload must contain exactly one nonempty English Tesseract traineddata file.' }
    Invoke-Native 'npm' @('--prefix', 'ui', 'ci')
    Invoke-Native 'npm' @('--prefix', 'ui', 'run', 'build')
    Invoke-Native 'cargo' @('xtask', 'gen-conf', '--target', $Target)
    $metadata = (& cargo metadata --no-deps --format-version 1 | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $version = @($metadata.packages | Where-Object { $_.name -eq 'convertme-gui' })[0].version
    if (-not $version) { throw 'Could not determine app version' }
    $cli = Join-Path $root 'ui/node_modules/.bin/tauri.cmd'
    Require-File $cli 'Local Tauri CLI'
    $buildStart = Get-Date
    Push-Location (Join-Path $root 'crates/convertme-gui')
    try { Invoke-Native $cli @('build', '--target', $Target, '--bundles', 'nsis') } finally { Pop-Location }
    $releaseDir = Join-Path $root "target/$Target/release"
    $exe = Join-Path $releaseDir 'convertme.exe'
    Require-File $exe 'Built application'
    $appMachine = Get-Machine $exe
    $expectedMachine = if ($arch -eq 'x64') { 'AMD64' } else { 'ARM64' }
    if ($appMachine -ne $expectedMachine) { throw "Application PE machine is $appMachine, expected $expectedMachine" }
    $installers = @(Get-ChildItem -LiteralPath (Join-Path $releaseDir 'bundle/nsis') -Filter '*.exe' -File | Where-Object { $_.LastWriteTime -ge $buildStart.AddSeconds(-2) })
    if ($installers.Count -ne 1) { throw "Expected one freshly built NSIS installer, found $($installers.Count)" }
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    $prefix = "ConvertMe-$version-windows-$arch"
    $setup = Join-Path $out "$prefix-setup.exe"
    $zip = Join-Path $out "$prefix-portable.zip"
    $sourcesZip = Join-Path $out "$prefix-sources-and-licenses.zip"
    Copy-Item -LiteralPath $installers[0].FullName -Destination $setup -Force
    $temp = Join-Path ([IO.Path]::GetTempPath()) ("convertme-package-" + [guid]::NewGuid().ToString('N'))
    try {
        $nsisContents = Join-Path $temp 'nsis'
        Invoke-Native (Join-Path $tools $manifest.executables.'7zz') @('x', '-y', "-o$nsisContents", $setup)
        $embeddedRuntime = @(Get-ChildItem -LiteralPath $nsisContents -Recurse -File -Filter 'convertme-webview2-runtime.exe')
        if ($embeddedRuntime.Count -ne 1 -or (Get-Sha256 $embeddedRuntime[0].FullName) -ne (Get-Sha256 $RuntimeInstaller)) { throw 'NSIS embedded runtime does not match the locked portable runtime' }
        $portable = Join-Path $temp 'ConvertMe'
        New-Item -ItemType Directory -Force -Path $portable, (Join-Path $portable 'runtime') | Out-Null
        Copy-Item -LiteralPath $exe -Destination $portable
        Get-ChildItem -LiteralPath $releaseDir -Filter '*.dll' -File | Copy-Item -Destination $portable
        Copy-Item -LiteralPath $tools -Destination (Join-Path $portable 'tools') -Recurse
        Copy-Item -LiteralPath $RuntimeInstaller -Destination (Join-Path $portable "runtime/$runtimeName")
        if ((Get-Sha256 (Join-Path $portable "runtime/$runtimeName")) -ne (Get-Sha256 $RuntimeInstaller)) { throw 'Portable runtime differs from locked installer' }
        Copy-Item -LiteralPath (Join-Path $root 'LICENSE'), (Join-Path $root 'NOTICE') -Destination $portable
        Copy-Item -LiteralPath $ComplianceDir -Destination (Join-Path $portable 'third-party') -Recurse
        @('Extract this directory before launching ConvertMe.', "If WebView2 is missing, run runtime\$runtimeName /silent /install unelevated as the current user, wait for it to finish, then run convertme.exe.", 'Settings and history are kept in the normal per-user app directories.') | Set-Content -LiteralPath (Join-Path $portable 'PORTABLE.txt') -Encoding UTF8
        Compress-Archive -LiteralPath $portable -DestinationPath $zip -Force
        Compress-Archive -LiteralPath $ComplianceDir -DestinationPath $sourcesZip -Force
    } finally { if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force } }
    Invoke-Native $Python @('scripts/vendor-windows.py', '--target', $Target, '--validate-payload', $tools)
    if ((Get-Sha256 $manifestPath) -ne $compliance.payload_manifest_sha256) { throw 'Payload changed during bundling' }
    Invoke-Native $Python @('scripts/release-inventory.py', 'create', '--target', $Target, '--artifacts', $out, '--payload', $tools, '--compliance', $ComplianceDir)
} finally {
    [Environment]::SetEnvironmentVariable('CONVERTME_WEBVIEW2_INSTALLER', $previousRuntime)
    [Environment]::SetEnvironmentVariable('CONVERTME_COMPLIANCE_DIR', $previousComplianceDir)
    Pop-Location
}
