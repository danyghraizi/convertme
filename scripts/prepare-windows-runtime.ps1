# Capture or retrieve the architecture-specific Microsoft Evergreen standalone runtime.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('x86_64-pc-windows-msvc','aarch64-pc-windows-msvc')][string] $Target,
    [Parameter(Mandatory = $true)][string] $Out,
    [switch] $CaptureLock,
    [string] $VerifyInstaller
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
$root = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $root 'packaging/windows/webview2.lock.json'
$arch = if ($Target -eq 'x86_64-pc-windows-msvc') { 'x64' } else { 'arm64' }
$name = if ($arch -eq 'x64') { 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe' } else { 'MicrosoftEdgeWebView2RuntimeInstallerARM64.exe' }
$origin = if ($arch -eq 'x64') { 'https://go.microsoft.com/fwlink/?linkid=2124701' } else { 'https://go.microsoft.com/fwlink/?linkid=2099616' }
function Inspect([string] $path) {
    $item = Get-Item -LiteralPath $path
    if ($item.Length -le 0 -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Invalid runtime file: $path" }
    $sig = Get-AuthenticodeSignature -LiteralPath $path
    if ($sig.Status -ne 'Valid' -or -not $sig.SignerCertificate -or $sig.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') { throw "Invalid Microsoft signature: $path" }
    return @{ sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant(); size = $item.Length; file_version = $item.VersionInfo.FileVersion; product_version = $item.VersionInfo.ProductVersion; signer_subject = $sig.SignerCertificate.Subject; signer_thumbprint = $sig.SignerCertificate.Thumbprint }
}
function Fetch([string] $url, [string] $dest) {
    $request = [Net.HttpWebRequest]::Create($url)
    $request.AllowAutoRedirect = $true
    $response = $request.GetResponse()
    try {
        $final = $response.ResponseUri.AbsoluteUri
        if ($response.ResponseUri.Scheme -ne 'https') { throw 'Runtime redirect did not end at HTTPS' }
        $inputStream = $response.GetResponseStream()
        $outputStream = [IO.File]::Open($dest, [IO.FileMode]::CreateNew)
        try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose(); $inputStream.Dispose() }
        return $final
    } finally { $response.Dispose() }
}
$lock = if (Test-Path -LiteralPath $lockPath) { Get-Content -Raw -LiteralPath $lockPath | ConvertFrom-Json } else { $null }
if ($lock -and $lock.schema_version -ne 1) { throw 'Invalid runtime lock schema' }
if ($VerifyInstaller) {
    if ($CaptureLock -or -not $lock) { throw 'Verification requires an existing lock and cannot capture' }
    $record = $lock.architectures.PSObject.Properties[$arch].Value
    if (-not $record -or $record.architecture -ne $arch) { throw "Missing runtime lock for $arch" }
    $info = Inspect (Resolve-Path -LiteralPath $VerifyInstaller).Path
    foreach ($key in @('sha256','size','file_version','product_version','signer_subject','signer_thumbprint')) { if ($info[$key] -ne $record.$key) { throw "Runtime $key does not match $arch lock" } }
    return
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$dest = Join-Path (Resolve-Path -LiteralPath $Out).Path $name
$temp = Join-Path (Resolve-Path -LiteralPath $Out).Path ('.runtime-' + [guid]::NewGuid().ToString('N'))
try {
    if ($CaptureLock) {
        $final = Fetch $origin $temp
        if ([IO.Path]::GetFileName(([Uri]$final).AbsolutePath) -ine $name) { throw "Microsoft redirect does not identify the expected standalone $arch installer: $final" }
        $info = Inspect $temp
        $record = [ordered]@{ architecture = $arch; origin_url = $origin; final_url = $final; file = $name; sha256 = $info.sha256; size = $info.size; file_version = $info.file_version; product_version = $info.product_version; signer_subject = $info.signer_subject; signer_thumbprint = $info.signer_thumbprint }
        $records = @{}
        if ($lock) { foreach ($p in $lock.architectures.PSObject.Properties) { $records[$p.Name] = $p.Value } }
        $records[$arch] = $record
        $newLock = @{ schema_version = 1; architectures = $records } | ConvertTo-Json -Depth 8
    } else {
        if (-not $lock) { throw 'Runtime lock missing; capture on a native Windows system first' }
        $record = $lock.architectures.PSObject.Properties[$arch].Value
        if (-not $record -or $record.architecture -ne $arch -or $record.final_url -notmatch '^https://') { throw "Missing or invalid $arch runtime lock" }
        if (Test-Path -LiteralPath $dest) { $temp = $dest } else { [void](Fetch $record.final_url $temp) }
        $info = Inspect $temp
        foreach ($key in @('sha256','size','file_version','product_version','signer_subject','signer_thumbprint')) { if ($info[$key] -ne $record.$key) { throw "Runtime $key does not match $arch lock" } }
    }
    if ($temp -ne $dest) {
        if (Test-Path -LiteralPath $dest) { [IO.File]::Replace($temp, $dest, $null) } else { [IO.File]::Move($temp, $dest) }
    }
    if ($CaptureLock) {
        $lockTemp = "$lockPath.$([guid]::NewGuid().ToString('N')).tmp"
        try {
            [IO.File]::WriteAllText($lockTemp, $newLock, (New-Object Text.UTF8Encoding $false))
            if (Test-Path -LiteralPath $lockPath) { [IO.File]::Replace($lockTemp, $lockPath, $null) } else { [IO.File]::Move($lockTemp, $lockPath) }
        } finally { if (Test-Path -LiteralPath $lockTemp) { Remove-Item -LiteralPath $lockTemp -Force } }
    }
    Write-Output $dest
} finally { if ($temp -ne $dest -and (Test-Path -LiteralPath $temp)) { Remove-Item -LiteralPath $temp -Force } }
