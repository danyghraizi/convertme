param(
  [string]$Cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$appRoot = Join-Path $repoRoot 'commercial-windows'
$rustup = Join-Path (Split-Path $Cargo) 'rustup.exe'
$installedToolchains = & $rustup toolchain list
$lockedToolchain = if ($installedToolchains -match '^1\.99\.1(?:-|\s|$)') { '1.99.1' } else { 'stable' }
$previousRustup = $env:RUSTUP_TOOLCHAIN
$env:RUSTUP_TOOLCHAIN = $lockedToolchain
$rustc = Join-Path (Split-Path $Cargo) 'rustc.exe'
try {
  $rustVersion = (& $rustc --version | Out-String).Trim()
  if ($rustVersion -notmatch '^rustc 1\.99\.1(?:\s|$)') { throw "Expected Rust 1.99.1, found $rustVersion." }
}
finally {
  if ($null -eq $previousRustup) { Remove-Item Env:RUSTUP_TOOLCHAIN -ErrorAction SilentlyContinue }
  else { $env:RUSTUP_TOOLCHAIN = $previousRustup }
}
$sourceLock = Get-Content -Raw (Join-Path $repoRoot 'packaging\sources.lock.json') | ConvertFrom-Json
$cargoAboutPin = $sourceLock.cargo_about.binaries.'windows-x86_64'
$cargoAboutPath = Join-Path $repoRoot ("vendor\source-inputs\" + $cargoAboutPin.binary.Replace('/', '\'))
$cargoAboutHash = (Get-FileHash -LiteralPath $cargoAboutPath -Algorithm SHA256 -ErrorAction SilentlyContinue).Hash.ToLowerInvariant()
if ($cargoAboutHash -ne $cargoAboutPin.sha256) {
  $python = Get-Command py.exe -ErrorAction SilentlyContinue
  if (-not $python) { throw 'The pinned cargo-about binary is missing or has a bad hash, and Python launcher py.exe is unavailable to restore it.' }
  & $python.Source -3 (Join-Path $repoRoot 'scripts\prepare-cargo-about.py') --host windows-x86_64 --out (Join-Path $repoRoot 'vendor\source-inputs')
  if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the pinned cargo-about binary.' }
  $cargoAboutHash = (Get-FileHash -LiteralPath $cargoAboutPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($cargoAboutHash -ne $cargoAboutPin.sha256) { throw 'The fetched cargo-about binary did not match its pinned hash.' }
}
$version = (& $cargoAboutPath --version | Out-String).Trim()
if ($version -ne "cargo-about $($sourceLock.cargo_about.version)") { throw "Unexpected cargo-about version: $version" }
$wrapperSource = Join-Path $appRoot 'packaging\cargo-filter-wrapper.rs'
$wrapperDir = Join-Path $appRoot 'target\notice-tools'
$wrapperExe = Join-Path $wrapperDir 'cargo.exe'
New-Item -ItemType Directory -Force -Path $wrapperDir | Out-Null

& (Join-Path (Split-Path $Cargo) 'rustc.exe') $wrapperSource -o $wrapperExe
if ($LASTEXITCODE -ne 0) { throw 'Could not build the target-filtering Cargo metadata wrapper.' }

$oldPath = $env:Path
$oldOffline = $env:CARGO_NET_OFFLINE
$oldCargo = $env:CONVERTME_REAL_CARGO
try {
  $env:Path = "$wrapperDir;$(Split-Path $Cargo);$oldPath"
  $env:CARGO_NET_OFFLINE = 'true'
  $env:CONVERTME_REAL_CARGO = $Cargo
  $env:RUSTUP_TOOLCHAIN = $lockedToolchain
  Push-Location $repoRoot
  try {
    & $cargoAboutPath generate `
      --manifest-path 'commercial-windows\Cargo.toml' `
      --config 'commercial-windows\packaging\cargo-about.toml' `
      --locked --offline --target x86_64-pc-windows-msvc `
      --output-file 'commercial-windows\THIRD_PARTY_NOTICES.md' `
      'commercial-windows\packaging\cargo-about.hbs'
    if ($LASTEXITCODE -ne 0) { throw "cargo-about failed with exit code $LASTEXITCODE." }
  }
  finally { Pop-Location }

  $sysroot = (& $rustc --print sysroot | Out-String).Trim()
  $licenseDir = Join-Path $sysroot 'share\doc\rust\licenses'
  foreach ($license in @(@{ Name = 'Apache-2.0'; File = 'Apache-2.0.txt' }, @{ Name = 'MIT'; File = 'MIT.txt' })) {
    $licensePath = Join-Path $licenseDir $license.File
    if (-not (Test-Path -LiteralPath $licensePath)) { throw "Rust standard library license file is missing: $licensePath" }
  }
  Add-Content -LiteralPath (Join-Path $appRoot 'THIRD_PARTY_NOTICES.md') -Encoding utf8 -Value @"

## Rust standard library (Rust 1.99.1)

The Windows executable statically includes Rust standard library code, distributed under Apache-2.0 or MIT. The corresponding toolchain license texts follow.

### Apache-2.0

````text
$(Get-Content -Raw (Join-Path $licenseDir 'Apache-2.0.txt'))
````

### MIT

````text
$(Get-Content -Raw (Join-Path $licenseDir 'MIT.txt'))
````
"@

  # cargo-about preserves trailing spaces from upstream license files. Normalize
  # the generated notice documents so diffs stay clean and deterministic.
  $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
  foreach ($noticePath in @(
    (Join-Path $appRoot 'THIRD_PARTY_NOTICES.md'),
    (Join-Path $appRoot 'packaging\THIRD_PARTY_SOURCE_NOTICES.md')
  )) {
    if (Test-Path -LiteralPath $noticePath) {
      $lines = [System.IO.File]::ReadAllLines($noticePath)
      $normalizedLines = foreach ($line in $lines) { $line.TrimEnd() }
      [System.IO.File]::WriteAllLines($noticePath, [string[]]$normalizedLines, $utf8NoBom)
    }
  }
}
finally {
  $env:Path = $oldPath
  if ($null -eq $oldOffline) { Remove-Item Env:CARGO_NET_OFFLINE -ErrorAction SilentlyContinue }
  else { $env:CARGO_NET_OFFLINE = $oldOffline }
  if ($null -eq $oldCargo) { Remove-Item Env:CONVERTME_REAL_CARGO -ErrorAction SilentlyContinue }
  else { $env:CONVERTME_REAL_CARGO = $oldCargo }
  if ($null -eq $oldRustup) { Remove-Item Env:RUSTUP_TOOLCHAIN -ErrorAction SilentlyContinue }
  else { $env:RUSTUP_TOOLCHAIN = $oldRustup }
}
