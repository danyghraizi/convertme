param(
    [Parameter(Mandatory=$true)][string]$Candidate,
    [Parameter(Mandatory=$true)][string]$Package,
    [Parameter(Mandatory=$true)][string]$Fixtures,
    [Parameter(Mandatory=$true)][string]$Out
)
$ErrorActionPreference = 'Stop'
$scenarioIds = @('build-identity','convert-image','convert-video','convert-audio','compress-scale','multi-file','multi-file-per-file','negative-cancel','negative-overwrite','persistence','pdf-organize','dialogs-images','dialogs-video','dialogs-audio','dialogs-pdf','instance-forward','crop-placement','annotation-pixels','video-trim','audio-trim')
$manualDescriptions = [ordered]@{
    'explorer-drop-and-modifiers' = 'Explorer drag/drop and modifiers, including exact paths and resulting outputs'
    'audible-trim-playback' = 'Audible speech on the actual output device, trim 2–4 seconds, play, pause, resume, close and replacement'
    'multi-monitor-dpi' = '100%, 150%, 200% monitor geometry, arrangement, clipping and negative-origin wheel clamp'
    'shell-clipboard-autostart' = 'Shell integration, clipboard and autostart on the dedicated account'
    'offline-install-lifecycle' = 'Disconnected runtime absent/present install, conversion, reinstall, upgrade/downgrade, running instance and uninstall; user data and shared runtime retained'
    'cross-volume-unc' = 'Actual cross-volume and UNC path operation and outputs'
}
function Hash([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Json([string]$Path) { return Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json }
function Ref([string]$Base, [string]$Relative) {
    $path = Join-Path $Base $Relative
    return @{ path = $Relative.Replace('\','/'); sha256 = (Hash $path) }
}
function RelativeFile([string]$Root, [string]$Name) {
    if (-not $Name -or $Name.Contains('\') -or $Name.Contains(':') -or $Name.StartsWith('/') -or @($Name.Split('/') | Where-Object { $_ -eq '' -or $_ -eq '.' -or $_ -eq '..' }).Count) { throw "Unsafe relative path: $Name" }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    $current = $rootFull
    foreach ($part in $Name.Split('/')) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            if ((Get-Item -Force -LiteralPath $current).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse point not allowed: $current" }
        }
    }
    $full = [IO.Path]::GetFullPath($current)
    if (-not $full.StartsWith($rootFull, [StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Missing or escaping file: $Name" }
    return $full
}
function Expand-SafeZip([string]$Zip, [string]$Destination) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($Zip)
    try {
        $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
        $base = [IO.Path]::GetFullPath($Destination).TrimEnd('\') + '\'
        foreach ($entry in $archive.Entries) {
            $name = $entry.FullName
            $trimmed = $name.TrimEnd('/')
            if (-not $trimmed -or $name.Contains('\') -or $name.Contains(':') -or $name.StartsWith('/') -or @($trimmed.Split('/') | Where-Object { $_ -eq '' -or $_ -eq '.' -or $_ -eq '..' -or $_.EndsWith('.') -or $_.EndsWith(' ') }).Count) { throw "Unsafe ZIP member: $name" }
            if (-not $seen.Add($trimmed)) { throw "Duplicate ZIP member: $name" }
            $mode = ($entry.ExternalAttributes -shr 16) -band 0xF000
            if ($mode -eq 0xA000 -or ($entry.ExternalAttributes -band 0x400)) { throw "Link or reparse ZIP member: $name" }
            $dest = [IO.Path]::GetFullPath((Join-Path $Destination $trimmed.Replace('/','\')))
            if (-not $dest.StartsWith($base, [StringComparison]::OrdinalIgnoreCase)) { throw "Escaping ZIP member: $name" }
        }
        foreach ($entry in $archive.Entries) {
            $dest = Join-Path $Destination $entry.FullName.Replace('/','\')
            if ($entry.FullName.EndsWith('/')) { New-Item -ItemType Directory -Force -Path $dest | Out-Null; continue }
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $dest, $false)
        }
    } finally { $archive.Dispose() }
}
function Write-Report($Report, [string]$Directory) {
    $Report | ConvertTo-Json -Depth 40 | Set-Content -LiteralPath (Join-Path $Directory 'report.json') -Encoding UTF8
}

$fullOut = [IO.Path]::GetFullPath($Out)
if (Test-Path -LiteralPath $fullOut) {
    if (-not (Test-Path -LiteralPath $fullOut -PathType Container) -or @(Get-ChildItem -LiteralPath $fullOut -Force).Count) { throw "Output must be a new or empty directory: $fullOut" }
} else { New-Item -ItemType Directory -Path $fullOut | Out-Null }
$report = [ordered]@{
    schema_version = 1; suite = 'Full'; selectors = @{}; required_scenarios = $scenarioIds
    results = @(); manual = @(); full_desktop_acceptance = $false; status = 'blocked'
    candidate_sha256 = $null; fixtures_manifest_sha256 = $null; fixtures_manifest = $null
    package = $null; cell = $null; os = $null; account = $null; executable = $null
    cli_report = $null; gui_report = $null; desktop_metadata = $null; reason = $null
}
try {
    $Candidate = (Resolve-Path -LiteralPath $Candidate).Path
    $Package = (Resolve-Path -LiteralPath $Package).Path
    $Fixtures = (Resolve-Path -LiteralPath $Fixtures).Path
    $candidateDoc = Json $Candidate
    if ($candidateDoc.schema_version -ne 1 -or $candidateDoc.version -ne '0.1.1') { throw 'Unsupported candidate schema or version' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run as a non-elevated standard user' }
    $system = Get-CimInstance Win32_OperatingSystem
    if ($system.ProductType -ne 1) { throw 'Windows Server is not a desktop acceptance cell' }
    $build = [int]$system.BuildNumber
    if ($build -ge 26100) { $osCell = 'windows11' } elseif ($build -ge 19045 -and $build -lt 22000) { $osCell = 'windows10' } else { throw "Unsupported Windows desktop build: $build" }
    $native = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $arch = switch ($native.ToUpperInvariant()) { 'AMD64' { 'x64' } 'ARM64' { 'arm64' } default { throw "Unsupported native architecture: $native" } }
    if ($osCell -eq 'windows10' -and $arch -eq 'arm64') { throw 'Windows 10 ARM64 is not an acceptance cell' }
    $kind = if ($Package.EndsWith('-portable.zip', [StringComparison]::OrdinalIgnoreCase)) { 'zip' } elseif ($Package.EndsWith('-setup.exe', [StringComparison]::OrdinalIgnoreCase)) { 'nsis' } else { throw 'Package must be the candidate portable ZIP or NSIS setup' }
    $target = if ($arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
    $packageHash = Hash $Package
    $asset = @($candidateDoc.assets | Where-Object { $_.target -eq $target -and [IO.Path]::GetFileName($_.path) -eq [IO.Path]::GetFileName($Package) -and $_.sha256 -eq $packageHash -and $_.size -eq (Get-Item -LiteralPath $Package).Length })
    if ($asset.Count -ne 1) { throw 'Package is not the exact candidate asset for this architecture' }
    $inventoryMatches = @()
    foreach ($item in $candidateDoc.inventories) {
        $file = Join-Path (Split-Path -Parent $Package) ([IO.Path]::GetFileName($item.path))
        if ((Test-Path -LiteralPath $file -PathType Leaf) -and (Hash $file) -eq $item.sha256) {
            $doc = Json $file
            if ($doc.target -eq $target) { $inventoryMatches += $doc }
        }
    }
    if ($inventoryMatches.Count -ne 1) { throw 'Place the exact candidate target inventory beside the package' }
    $inventory = $inventoryMatches[0]
    if (@($inventory.artifacts | Where-Object { $_.file -eq [IO.Path]::GetFileName($Package) -and $_.sha256 -eq $packageHash }).Count -ne 1) { throw 'Package does not match inventory' }
    if (-not $inventory.app.sha256 -or -not $inventory.app.ui_build) { throw 'Inventory lacks executable hash or UI identity' }
    $fixtureManifest = RelativeFile $Fixtures 'input-hashes.json'
    if ((Hash $fixtureManifest) -ne $candidateDoc.fixtures_manifest_sha256) { throw 'Fixture manifest differs from candidate' }
    $fixtureDoc = Json $fixtureManifest
    foreach ($entry in $fixtureDoc.PSObject.Properties) {
        $source = RelativeFile $Fixtures $entry.Name
        if ((Hash $source) -ne $entry.Value) { throw "Fixture hash mismatch: $($entry.Name)" }
    }
    Copy-Item -LiteralPath $fixtureManifest -Destination (Join-Path $fullOut 'input-hashes.json')
    $report.candidate_sha256 = Hash $Candidate
    $report.fixtures_manifest_sha256 = $candidateDoc.fixtures_manifest_sha256
    $report.fixtures_manifest = Ref $fullOut 'input-hashes.json'
    $report.package = @{ file = $asset[0].path; sha256 = $packageHash; kind = $kind }
    $report.cell = "$osCell-$arch-$kind"
    $report.os = @{ edition = $system.Caption; build = $system.BuildNumber; native_arch = $arch }
    $report.account = @{ sid = $identity.User.Value; elevated = $false; session = [string](Get-Process -Id $PID).SessionId }
    $destination = Join-Path $fullOut 'package'
    New-Item -ItemType Directory -Path $destination | Out-Null
    if ($kind -eq 'zip') {
        Expand-SafeZip $Package $destination
    } else {
        $existing = Join-Path $env:LOCALAPPDATA 'Programs\ConvertMe'
        $appData = Join-Path $env:APPDATA 'ConvertMe'
        if ((Test-Path -LiteralPath $existing) -or (Test-Path -LiteralPath $appData)) { throw 'Existing ConvertMe installation or profile detected; use a dedicated disposable account' }
        $confirmation = Read-Host 'NSIS modifies this user account and may install shared WebView2. Type DISPOSABLE to confirm this is an authorized disposable system/profile'
        if ($confirmation -cne 'DISPOSABLE') { throw 'NSIS installation not authorized on a disposable system/profile' }
        $installer = Start-Process -FilePath $Package -ArgumentList @('/S', "/D=$destination") -PassThru -Wait
        if ($installer.ExitCode -ne 0) { throw "Installer failed with exit code $($installer.ExitCode)" }
    }
    $executables = @(Get-ChildItem -LiteralPath $destination -Recurse -File -Filter 'convertme.exe')
    if ($executables.Count -ne 1) { throw 'Package did not produce exactly one convertme.exe' }
    $binary = $executables[0].FullName
    $binaryHash = Hash $binary
    if ($binaryHash -ne $inventory.app.sha256) { throw 'Extracted/installed executable does not match candidate inventory' }
    if (-not $inventory.payload_files -or -not @($inventory.payload_files.PSObject.Properties).Count) { throw 'Inventory lacks payload file hashes' }
    $toolsRoot = Join-Path (Split-Path -Parent $binary) 'tools'
    foreach ($entry in $inventory.payload_files.PSObject.Properties) {
        $payloadFile = RelativeFile $toolsRoot $entry.Name
        if ((Hash $payloadFile) -ne $entry.Value) { throw "Installed payload hash mismatch: $($entry.Name)" }
    }
    $report.executable = @{ sha256 = $binaryHash; ui_build = $inventory.app.ui_build }
    $cliDir = Join-Path $fullOut 'cli'
    $guiDir = Join-Path $fullOut 'gui'
    & python (Join-Path $PSScriptRoot 'quality-audit.py') --binary $binary --fixtures $Fixtures --release --out $cliDir
    $cliExit = $LASTEXITCODE
    & powershell.exe -NoProfile -File (Join-Path $PSScriptRoot 'app-e2e-windows.ps1') -Binary $binary -Fixtures $Fixtures -Suite Full -Out $guiDir
    $guiExit = $LASTEXITCODE
    if (-not (Test-Path -LiteralPath (Join-Path $cliDir 'report.json') -PathType Leaf)) { throw "CLI audit did not write its report (exit $cliExit)" }
    if (-not (Test-Path -LiteralPath (Join-Path $guiDir 'report.json') -PathType Leaf)) { throw "UIA harness did not write its report (exit $guiExit)" }
    $report.cli_report = Ref $fullOut 'cli/report.json'
    $report.gui_report = Ref $fullOut 'gui/report.json'
    $gui = Json (Join-Path $guiDir 'report.json')
    $observedBuild = [string]$gui.metadata.ui_build
    if (-not $observedBuild.Contains([string]$inventory.app.ui_build)) { throw 'UIA-observed interface build differs from inventory' }
    if ($gui.metadata.binary_sha256 -ne $binaryHash -or $gui.metadata.fixtures_manifest_sha256 -ne $candidateDoc.fixtures_manifest_sha256) { throw 'UIA report binary or fixture mismatch' }
    $report.desktop_metadata = $gui.metadata
    foreach ($id in $scenarioIds) {
        $matches = @($gui.results | Where-Object { $_.id -eq $id })
        if ($matches.Count -ne 1) { $report.results += @{ id = $id; status = 'blocked'; measurements = @{}; evidence_files = @(); reason = 'Missing or duplicate UIA result' }; continue }
        $row = $matches[0]
        $refs = @()
        foreach ($item in $row.evidence_files) {
            $file = RelativeFile $guiDir $item.path
            if ((Hash $file) -ne $item.sha256) { throw "UIA evidence hash mismatch: $($item.path)" }
            $refs += @{ path = 'gui/' + $item.path; sha256 = $item.sha256.ToLowerInvariant() }
        }
        $report.results += @{ id = $id; status = $row.status; measurements = $row.measurements; evidence_files = $refs; reason = $row.reason }
    }
    Write-Host 'Manual acceptance requires actual observations and retained evidence; press Enter to leave any item blocked.'
    foreach ($id in $manualDescriptions.Keys) {
        Write-Host "`n${id}: $($manualDescriptions[$id])"
        $observation = Read-Host 'Describe what you personally observed (empty = blocked)'
        if ([string]::IsNullOrWhiteSpace($observation)) { $report.manual += @{ id = $id; status = 'blocked'; observations = @{}; evidence_files = @() }; continue }
        $observer = Read-Host 'Observer name'
        $file = Read-Host 'Path to an existing evidence file (screenshot/log)'
        $answer = Read-Host 'Did every described requirement pass? Type PASS only for a complete pass'
        if ([string]::IsNullOrWhiteSpace($observer) -or -not (Test-Path -LiteralPath $file -PathType Leaf) -or $answer -cne 'PASS') { $report.manual += @{ id = $id; status = 'blocked'; observations = @{ description = $observation }; evidence_files = @() }; continue }
        $source = Get-Item -LiteralPath $file -Force
        if ($source.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Manual evidence is a reparse point: $file" }
        $manualDir = Join-Path $fullOut 'manual'
        New-Item -ItemType Directory -Force -Path $manualDir | Out-Null
        $name = $id + '-' + $source.Name
        Copy-Item -LiteralPath $source.FullName -Destination (Join-Path $manualDir $name)
        $report.manual += @{ id = $id; status = 'passed'; observer = $observer; observed_at = [DateTimeOffset]::Now.ToString('o'); observations = @{ description = $observation }; evidence_files = @((Ref $fullOut ('manual/' + $name))) }
    }
    $success = $cliExit -eq 0 -and $guiExit -eq 0 -and $gui.full_automated_acceptance -eq $true -and @($report.results | Where-Object { $_.status -ne 'passed' }).Count -eq 0 -and @($report.manual | Where-Object { $_.status -ne 'passed' }).Count -eq 0
    $report.full_desktop_acceptance = $success
    $report.status = if ($success) { 'passed' } else { 'blocked' }
} catch {
    $report.status = 'blocked'
    $report.reason = $_.Exception.Message
    Write-Error -ErrorAction Continue "Acceptance blocked: $($_.Exception.Message)"
} finally {
    Write-Report $report $fullOut
}
if ($report.status -ne 'passed') { exit 1 }
