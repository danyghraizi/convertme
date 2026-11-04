param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$Fixtures,
    [Parameter(Mandatory=$true)][string]$Out,
    [string]$Only = '',
    [ValidateSet('Smoke', 'Full')][string]$Suite = 'Smoke'
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ConvertMePointer {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
}
'@
$Binary = (Resolve-Path -LiteralPath $Binary).Path
$Fixtures = (Resolve-Path -LiteralPath $Fixtures).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$fullFixtures = @(
    'images/poster-portrait.png', 'images/photo-landscape-4000x3000.jpg',
    'images/screenshot-text.png', 'images/tiny-8x8.png',
    'video/clip-landscape-720p-10s.mp4', 'audio/speech.mp3',
    'audio/speech.flac', 'pdf/report-5-pages.pdf'
)
$fullScenarioIds = @(
    'build-identity', 'convert-image', 'convert-video', 'convert-audio', 'compress-scale',
    'multi-file', 'multi-file-per-file', 'negative-cancel', 'negative-overwrite',
    'persistence', 'pdf-organize', 'dialogs-images', 'dialogs-video', 'dialogs-audio',
    'dialogs-pdf', 'instance-forward', 'crop-placement', 'annotation-pixels',
    'video-trim', 'audio-trim'
)
$work = Join-Path $Out ('convert-image-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
$inputFile = Join-Path $work 'photo.png'
$profile = Join-Path $work 'profile'
New-Item -ItemType Directory -Force -Path (Join-Path $profile 'roaming'), (Join-Path $profile 'local') | Out-Null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$env:APPDATA = Join-Path $profile 'roaming'
$env:LOCALAPPDATA = Join-Path $profile 'local'
$results = [System.Collections.Generic.List[object]]::new()
$metadata = [ordered]@{ binary = $Binary; binary_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Binary).Hash; os = $null; account = $null; machine = $env:PROCESSOR_ARCHITECTURE; pe_machine = $null; webview2 = @(); helpers = @(); ui_build = $null; app_probe = $null; fixtures_manifest_sha256 = $null; monitors = @() }
function Get-PeMachine([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = New-Object System.IO.BinaryReader($stream)
        if ($reader.ReadUInt16() -ne 0x5a4d) { throw "Not a PE file: $Path" }
        $stream.Position = 0x3c; $offset = $reader.ReadUInt32(); $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550) { throw "Invalid PE signature: $Path" }
        return ('0x{0:X4}' -f $reader.ReadUInt16())
    } finally { $stream.Dispose() }
}
function Snapshot([System.Windows.Automation.AutomationElement]$Root) {
    $elements = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $rows = @()
    foreach ($item in $elements) {
        try {
            $r = $item.Current.BoundingRectangle
            $rows += [ordered]@{ name = $item.Current.Name; type = $item.Current.ControlType.ProgrammaticName; enabled = $item.Current.IsEnabled; x = $r.X; y = $r.Y; width = $r.Width; height = $r.Height }
        } catch { }
    }
    return ,$rows
}
function Screenshot([string]$Path) {
    $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try { $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size); $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function Find-Button($Root, [string]$Name) {
    $nodes = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($node in $nodes) {
        try { if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $node.Current.Name -eq $Name -and $node.Current.IsEnabled) { return $node } } catch { }
    }
    return $null
}
function Click-Element($Element) {
    $rect = $Element.Current.BoundingRectangle
    if ($rect.IsEmpty -or $rect.Width -le 0 -or $rect.Height -le 0) { throw 'UIA element has no clickable bounds' }
    if (-not [ConvertMePointer]::SetCursorPos([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2))) { throw 'Could not position pointer for UIA element' }
    [ConvertMePointer]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [ConvertMePointer]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}
function Invoke-MediaScenario([string]$Id, [string]$Relative, [string]$Target, [string]$Extension, [hashtable]$Expected) {
    $directory = Join-Path $Out $Id
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $source = Join-Path $Fixtures $Relative
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixtures 'input-hashes.json') | ConvertFrom-Json
    $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash
    if (-not $manifest.$Relative -or $manifest.$Relative -ne $sourceHash) { throw "Fixture digest mismatch: $Relative" }
    $input = Join-Path $directory (Split-Path -Leaf $source)
    Copy-Item -LiteralPath $source -Destination $input
    $child = $null
    $savedPath = $env:PATH; $savedTools = $env:CONVERTME_TOOLS_DIR
    try {
        $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $child = Start-Process -FilePath $Binary -ArgumentList ('"' + $input + '"') -WorkingDirectory $directory -PassThru
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
        $button = $null; $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw "Application exited before $Id became ready" }
            if ($child.MainWindowHandle -ne 0) {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
                if ($root) { $button = Find-Button $root $Target; if ($button) { break } }
            }
            Start-Sleep -Milliseconds 250
        }
        if (-not $button) { throw "UIA target $Target unavailable" }
        Click-Element $button
        $output = $null; $deadline = [DateTime]::UtcNow.AddSeconds(240)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw "Application exited during $Id" }
            $output = Get-ChildItem -LiteralPath $directory -File -Filter "*.$Extension" | Select-Object -First 1
            if ($output) { Start-Sleep -Seconds 2; break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $output) { throw "No $Target output was published" }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $input).Hash -ne $sourceHash) { throw "Input changed during $Id" }
        $requestPath = Join-Path $directory 'assertion.json'
        @{ kind = 'media'; path = $output.FullName; binary = $Binary; expected = $Expected } |
            ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 -LiteralPath $requestPath
        $raw = & python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $requestPath
        if ($LASTEXITCODE -ne 0) { throw "$Id validation failed: $($raw | Out-String)" }
        $facts = ($raw | Out-String) | ConvertFrom-Json
        if (-not $facts.facts.audio -or ($Id -eq 'convert-video' -and -not $facts.facts.video)) { throw "$Id output lacks required media streams" }
        $shot = Join-Path $directory 'after.png'
        Screenshot $shot
        $results.Add(@{ id = $Id; name = $Id; status = 'passed'; measurements = @{ input_sha256 = $sourceHash; output_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $output.FullName).Hash; validator = $facts }; evidence_files = @(@{ path = "$Id/after.png"; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $shot).Hash.ToLowerInvariant() }) })
    } finally {
        if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue }
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
    }
}
function Invoke-CompressScale {
    $id = 'compress-scale'; $directory = Join-Path $Out $id
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $relative = 'images/poster-portrait.png'; $source = Join-Path $Fixtures $relative
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixtures 'input-hashes.json') | ConvertFrom-Json
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash
    if (-not $manifest.$relative -or $manifest.$relative -ne $digest) { throw "Fixture digest mismatch: $relative" }
    $inputPath = Join-Path $directory 'poster.png'
    Copy-Item -LiteralPath $source -Destination $inputPath
    $child = $null; $savedPath = $env:PATH; $savedTools = $env:CONVERTME_TOOLS_DIR
    try {
        $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $child = Start-Process -FilePath $Binary -ArgumentList ('"' + $inputPath + '"') -WorkingDirectory $directory -PassThru
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
        $button = $null; $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw 'Application exited before compression dialog' }
            if ($child.MainWindowHandle -ne 0) {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
                if ($root) {
                    $button = Find-Button $root 'Compress'
                    if (-not $button) { $button = Find-Button $root 'Compress to size' }
                    if ($button) { break }
                }
            }
            Start-Sleep -Milliseconds 250
        }
        if (-not $button) { throw 'Compress tool not available in UIA' }
        Click-Element $button
        $combo = $null; $deadline = [DateTime]::UtcNow.AddSeconds(30)
        while ([DateTime]::UtcNow -lt $deadline) {
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
            $nodes = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
            foreach ($node in $nodes) {
                if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::ComboBox -and $node.Current.Name -eq 'Image size') { $combo = $node; break }
            }
            if ($combo) { break }
            Start-Sleep -Milliseconds 250
        }
        if (-not $combo) { throw 'Image size select not exposed through UIA' }
        Click-Element $combo
        Start-Sleep -Milliseconds 300
        [Windows.Forms.SendKeys]::SendWait('{HOME}{DOWN}{DOWN}{ENTER}')
        Start-Sleep -Milliseconds 500
        $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
        $nodes = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $selected = @($nodes | Where-Object { $_.Current.Name -eq '50 % of the dimensions' })
        if (-not $selected.Count) { throw '50% option is not exposed after selection' }
        $actions = @($nodes | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -eq 'Compress' -and $_.Current.IsEnabled })
        if (-not $actions.Count) { throw 'Compression submit action unavailable' }
        Click-Element $actions[-1]
        $output = $null; $deadline = [DateTime]::UtcNow.AddSeconds(240)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw 'Application exited during compression' }
            $output = Get-ChildItem -LiteralPath $directory -File -Filter '*-compress*.png' | Select-Object -First 1
            if ($output) { Start-Sleep -Seconds 2; break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $output) { throw 'Compression did not publish an output' }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $inputPath).Hash -ne $digest) { throw 'Compression changed its source' }
        $image = [Drawing.Image]::FromFile($inputPath)
        try { $width = [int]($image.Width / 2); $height = [int]($image.Height / 2) } finally { $image.Dispose() }
        $requestPath = Join-Path $directory 'assertion.json'
        @{ kind = 'image'; path = $output.FullName; binary = $Binary; expected = @{ width = $width; height = $height; format = 'PNG' } } |
            ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -LiteralPath $requestPath
        $raw = & python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $requestPath
        if ($LASTEXITCODE -ne 0) { throw "Compression validation failed: $($raw | Out-String)" }
        $shot = Join-Path $directory 'after.png'; Screenshot $shot
        $results.Add(@{ id = $id; name = $id; status = 'passed'; measurements = @{ input_sha256 = $digest; output_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $output.FullName).Hash; validator = (($raw | Out-String) | ConvertFrom-Json) }; evidence_files = @(@{ path = "$id/after.png"; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $shot).Hash.ToLowerInvariant() }) })
    } finally {
        if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue }
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
    }
}
function Invoke-PdfOrganize {
    $id = 'pdf-organize'; $directory = Join-Path $Out $id
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $relative = 'pdf/report-5-pages.pdf'; $source = Join-Path $Fixtures $relative
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixtures 'input-hashes.json') | ConvertFrom-Json
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash
    if (-not $manifest.$relative -or $manifest.$relative -ne $digest) { throw "Fixture digest mismatch: $relative" }
    $inputPath = Join-Path $directory 'report.pdf'
    Copy-Item -LiteralPath $source -Destination $inputPath
    $child = $null; $savedPath = $env:PATH; $savedTools = $env:CONVERTME_TOOLS_DIR
    try {
        $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $child = Start-Process -FilePath $Binary -ArgumentList ('"' + $inputPath + '"') -WorkingDirectory $directory -PassThru
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
        $button = $null; $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw 'Application exited before PDF organizer' }
            if ($child.MainWindowHandle -ne 0) {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
                if ($root) { $button = Find-Button $root 'Organize pages'; if ($button) { break } }
            }
            Start-Sleep -Milliseconds 250
        }
        if (-not $button) { throw 'Organize pages tool not exposed through UIA' }
        Click-Element $button
        foreach ($label in @('Duplicate page 2', 'Remove page 4', 'Rotate page 1', 'Save Organized PDF')) {
            $button = $null; $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while ([DateTime]::UtcNow -lt $deadline) {
                $child.Refresh()
                if ($child.HasExited) { throw "Application exited before $label" }
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($child.MainWindowHandle)
                if ($root) { $button = Find-Button $root $label; if ($button) { break } }
                Start-Sleep -Milliseconds 250
            }
            if (-not $button) { throw "PDF organizer control unavailable: $label" }
            Click-Element $button
            Start-Sleep -Milliseconds 600
        }
        $output = $null; $deadline = [DateTime]::UtcNow.AddSeconds(240)
        while ([DateTime]::UtcNow -lt $deadline) {
            $child.Refresh()
            if ($child.HasExited) { throw 'Application exited during PDF organization' }
            $output = Get-ChildItem -LiteralPath $directory -File -Filter '*.pdf' | Where-Object { $_.FullName -ne $inputPath } | Select-Object -First 1
            if ($output) { Start-Sleep -Seconds 2; break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $output) { throw 'PDF organizer did not publish an output' }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $inputPath).Hash -ne $digest) { throw 'PDF organizer changed its source' }
        $requestPath = Join-Path $directory 'assertion.json'
        $pageMap = @(1, 2, 2, 3, 5); $sourcePages = @()
        for ($index = 0; $index -lt $pageMap.Count; $index++) { $sourcePages += @{ page = $index + 1; source_page = $pageMap[$index] } }
        @{ kind = 'pdf'; path = $output.FullName; source = $inputPath; binary = $Binary; expected = @{ pages = 5; source_pages = $sourcePages; page_rotations = @(@{ page = 1; rotation = 90 }) } } |
            ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -LiteralPath $requestPath
        $raw = & python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $requestPath
        if ($LASTEXITCODE -ne 0) { throw "PDF validation failed: $($raw | Out-String)" }
        $shot = Join-Path $directory 'after.png'; Screenshot $shot
        $results.Add(@{ id = $id; name = $id; status = 'passed'; measurements = @{ input_sha256 = $digest; output_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $output.FullName).Hash; validator = (($raw | Out-String) | ConvertFrom-Json) }; evidence_files = @(@{ path = "$id/after.png"; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $shot).Hash.ToLowerInvariant() }) })
    } finally {
        if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue }
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
    }
}
# Insert these functions before the main try block. They use the existing
# $Binary, $Fixtures, $Out, $results, Find-Button, Click-Element and Screenshot.
function New-MultiContext([string]$Id, [string[]]$Relatives) {
    $run = [guid]::NewGuid().ToString('N')
    $base = Join-Path (Join-Path $Out $Id) $run
    $workDir = Join-Path $base 'work'; $profileDir = Join-Path $base 'profile'
    New-Item -ItemType Directory -Force -Path $workDir, (Join-Path $profileDir 'roaming'), (Join-Path $profileDir 'local') | Out-Null
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixtures 'input-hashes.json') | ConvertFrom-Json
    $inputs = @(); $hashes = @{}
    foreach ($relative in $Relatives) {
        $source = Join-Path $Fixtures $relative
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant()
        if (-not $manifest.$relative -or $manifest.$relative -ne $hash) { throw "Fixture digest mismatch: $relative" }
        $dest = Join-Path $workDir (Split-Path -Leaf $source)
        Copy-Item -LiteralPath $source -Destination $dest
        $inputs += $dest; $hashes[$dest] = $hash
    }
    return @{ id=$Id; base=$base; work=$workDir; profile=$profileDir; inputs=$inputs; hashes=$hashes; evidencePrefix="$Id/$run" }
}
function Start-MultiApp($Context, [string[]]$Inputs) {
    $savedPath=$env:PATH; $savedTools=$env:CONVERTME_TOOLS_DIR; $savedApp=$env:APPDATA; $savedLocal=$env:LOCALAPPDATA
    try {
        $env:PATH=(Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $env:APPDATA=Join-Path $Context.profile 'roaming'; $env:LOCALAPPDATA=Join-Path $Context.profile 'local'
        $args = @($Inputs | ForEach-Object { '"' + $_ + '"' })
        if ($args.Count) { return Start-Process -FilePath $Binary -ArgumentList $args -WorkingDirectory $Context.work -PassThru }
        return Start-Process -FilePath $Binary -WorkingDirectory $Context.work -PassThru
    } finally {
        $env:PATH=$savedPath; $env:APPDATA=$savedApp; $env:LOCALAPPDATA=$savedLocal
        if ($null -eq $savedTools) { Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue } else { $env:CONVERTME_TOOLS_DIR=$savedTools }
    }
}
function Get-MultiRoot($Process) {
    $Process.Refresh()
    if ($Process.HasExited) { throw 'Application exited unexpectedly' }
    if ($Process.MainWindowHandle -eq 0) { return $null }
    return [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
}
function Find-MultiNode($Root, [string]$Name, $Type=$null) {
    if (-not $Root) { return $null }
    foreach ($node in $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
        if ($node.Current.Name -eq $Name -and ($null -eq $Type -or $node.Current.ControlType -eq $Type)) { return $node }
    }
    return $null
}
function Wait-MultiNode($Process, [string]$Name, $Type=$null, [int]$Seconds=30) {
    $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $node=Find-MultiNode (Get-MultiRoot $Process) $Name $Type
        if ($node -and $node.Current.IsEnabled) { return $node }
        Start-Sleep -Milliseconds 200
    }
    throw "UIA control unavailable: $Name"
}
function Wait-MultiText($Process, [string]$Text, [int]$Seconds=30) {
    $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $root=Get-MultiRoot $Process
        if ($root) {
            foreach ($node in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)) {
                if ($node.Current.Name -like "*$Text*") { return $node.Current.Name }
            }
        }
        Start-Sleep -Milliseconds 200
    }
    throw "UIA text unavailable: $Text"
}
function Click-MultiButton($Process, [string]$Name) {
    Click-Element (Wait-MultiNode $Process $Name ([System.Windows.Automation.ControlType]::Button))
}
function Set-MultiCheck($Process, [string]$Name, [bool]$Checked) {
    $node=Wait-MultiNode $Process $Name ([System.Windows.Automation.ControlType]::CheckBox)
    $pattern=$node.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    $want=if ($Checked) { [System.Windows.Automation.ToggleState]::On } else { [System.Windows.Automation.ToggleState]::Off }
    if ($pattern.Current.ToggleState -ne $want) { $pattern.Toggle() }
    Start-Sleep -Milliseconds 300
    $node=Wait-MultiNode $Process $Name ([System.Windows.Automation.ControlType]::CheckBox)
    if ($node.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Current.ToggleState -ne $want) { throw "Checkbox did not reach requested state: $Name" }
}
function Get-MultiSelection($Process, [string]$Name) {
    $combo=Wait-MultiNode $Process $Name ([System.Windows.Automation.ControlType]::ComboBox)
    $selection=$combo.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()
    if ($selection.Count -ne 1) { throw "UIA combo has no unique selection: $Name" }
    return $selection[0].Current.Name
}
function Set-MultiSelection($Process, [string]$Name, [string]$Wanted, [int]$Index) {
    $combo=Wait-MultiNode $Process $Name ([System.Windows.Automation.ControlType]::ComboBox)
    Click-Element $combo
    [Windows.Forms.SendKeys]::SendWait('{HOME}' + ('{DOWN}' * $Index) + '{ENTER}')
    Start-Sleep -Milliseconds 400
    if ((Get-MultiSelection $Process $Name) -ne $Wanted) { throw "Combo $Name did not select $Wanted" }
}
function Open-MultiActivity($Process) {
    $root=Get-MultiRoot $Process
    foreach ($node in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)) {
        if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $node.Current.Name -like 'Activity*') {
            $expand=$node.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
            if ($expand.Current.ExpandCollapseState -ne [System.Windows.Automation.ExpandCollapseState]::Expanded) { Click-Element $node; Start-Sleep -Milliseconds 400 }
            return
        }
    }
    throw 'Activity button unavailable via UIA'
}
function Get-MultiHistory($Process) {
    $panel=Wait-MultiNode $Process 'Job history' $null 15
    $rows=@()
    foreach ($node in $panel.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)) {
        if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::ListItem) {
            $parts=@($node.Current.Name)
            foreach ($item in $node.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)) { if ($item.Current.Name) { $parts += $item.Current.Name } }
            $rows += ($parts -join ' ')
        }
    }
    return ,$rows
}
function Wait-MultiHistory($Process, [string]$Status, [int]$Count=1, [int]$Seconds=240) {
    $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $rows=@(Get-MultiHistory $Process)
        if (@($rows | Where-Object { $_ -match "(?i)\b$Status\b" }).Count -ge $Count) { return ,$rows }
        Start-Sleep -Milliseconds 250
    }
    throw "History did not show $Count $Status row(s)"
}
function Open-MultiCompress($Process) {
    Click-MultiButton $Process 'Compress'
    $null=Wait-MultiNode $Process 'Image size' ([System.Windows.Automation.ControlType]::ComboBox)
}
function Submit-MultiCompress($Process) {
    $root=Get-MultiRoot $Process
    $buttons=@($root.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -eq 'Compress' -and $_.Current.IsEnabled })
    if ($buttons.Count -lt 2) { throw 'Compress dialog submit button unavailable through UIA' }
    Click-Element $buttons[-1]
}
function Assert-MultiInputs($Context) {
    foreach ($path in $Context.inputs) {
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -ne $Context.hashes[$path]) { throw "Input changed: $path" }
    }
}
function Assert-MultiImage($Context, [string]$Path, [int]$Width, [int]$Height, [string]$Format, [string]$Label) {
    $request=Join-Path $Context.base ("assert-$Label.json")
    @{kind='image'; path=$Path; binary=$Binary; expected=@{width=$Width; height=$Height; format=$Format}} | ConvertTo-Json -Depth 10 | Set-Content -Encoding UTF8 -LiteralPath $request
    $raw=& python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $request
    if ($LASTEXITCODE -ne 0) { throw "Output validation failed for $Path`: $($raw | Out-String)" }
    $facts=($raw | Out-String) | ConvertFrom-Json
    if (-not $facts.passed) { throw "Output contract rejected $Path" }
    return $facts
}
function Save-MultiResult($Context, $Measurements, [string[]]$Shots) {
    $evidence=@()
    foreach ($shot in $Shots) {
        $path=Join-Path $Context.base $shot
        $evidence += @{path="$($Context.evidencePrefix)/$shot"; sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()}
    }
    $results.Add(@{id=$Context.id; name=$Context.id; status='passed'; measurements=$Measurements; evidence_files=$evidence})
}
function Stop-MultiApp($Process) {
    if ($Process) { $Process.Refresh(); if (-not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue; $Process.WaitForExit() } }
}
function Invoke-MultiFileScenario([bool]$PerFile) {
    $id=if ($PerFile) {'multi-file-per-file'} else {'multi-file'}
    $second=if ($PerFile) {'images/photo-landscape-4000x3000.jpg'} else {'images/screenshot-text.png'}
    $ctx=New-MultiContext $id @('images/poster-portrait.png',$second); $child=$null
    try {
        $child=Start-MultiApp $ctx $ctx.inputs
        $null=Wait-MultiNode $child 'Compress' ([System.Windows.Automation.ControlType]::Button) 60
        Open-MultiActivity $child
        Open-MultiCompress $child
        $first=Wait-MultiText $child '1 of 2'
        if (-not $PerFile) {
            Click-MultiButton $child 'Next file'; $secondText=Wait-MultiText $child '2 of 2'
            Click-MultiButton $child 'Previous file'; $null=Wait-MultiText $child '1 of 2'
            Set-MultiCheck $child 'Same settings for every file' $true
            Set-MultiSelection $child 'Image size' '50 % of the dimensions' 2
        } else {
            Set-MultiCheck $child 'Same settings for every file' $false
            Set-MultiSelection $child 'Image size' '50 % of the dimensions' 2
            Click-MultiButton $child 'Next file'; $secondText=Wait-MultiText $child '2 of 2'
            if ((Get-MultiSelection $child 'Image size') -ne 'Keep original dimensions') { throw 'Second file did not retain independent default' }
            Set-MultiSelection $child 'Image size' '25 % of the dimensions' 3
        }
        Screenshot (Join-Path $ctx.base 'dialog.png')
        Submit-MultiCompress $child
        $history=Wait-MultiHistory $child 'done' 2
        $outputs=@(Get-ChildItem -LiteralPath $ctx.work -File | Where-Object { $_.Name -match '-compress' })
        if ($outputs.Count -ne 2) { throw "Expected exactly two compressed outputs, got $($outputs.Count)" }
        $poster=@($outputs | Where-Object { $_.Name -like 'poster-portrait-compress*' })
        $other=@($outputs | Where-Object { $_.Name -like (([IO.Path]::GetFileNameWithoutExtension($ctx.inputs[1])) + '-compress*') })
        if ($poster.Count -ne 1 -or $other.Count -ne 1) { throw 'Compressed output names do not match inputs' }
        $p=Assert-MultiImage $ctx $poster[0].FullName 1000 1500 'PNG' 'poster'
        if ($PerFile) { $o=Assert-MultiImage $ctx $other[0].FullName 1000 750 'JPEG' 'other' }
        else { $o=Assert-MultiImage $ctx $other[0].FullName 800 500 'PNG' 'other' }
        Assert-MultiInputs $ctx
        Screenshot (Join-Path $ctx.base 'after.png')
        Save-MultiResult $ctx @{input_sha256=$ctx.hashes; output_sha256=@((Get-FileHash $poster[0].FullName -Algorithm SHA256).Hash,(Get-FileHash $other[0].FullName -Algorithm SHA256).Hash); first_file=$first; second_file=$secondText; history=$history; validators=@($p,$o)} @('dialog.png','after.png')
    } finally { Stop-MultiApp $child }
}
function Invoke-NegativeCancelScenario {
    $ctx=New-MultiContext 'negative-cancel' @('images/poster-portrait.png'); $child=$null
    try {
        $child=Start-MultiApp $ctx $ctx.inputs
        $null=Wait-MultiNode $child 'Compress' ([System.Windows.Automation.ControlType]::Button) 60
        Open-MultiActivity $child; Open-MultiCompress $child
        Set-MultiCheck $child 'Reduce colours (lossy)' $true
        Set-MultiCheck $child 'Compress to a target file size' $true
        $entry=Wait-MultiNode $child 'Target size' ([System.Windows.Automation.ControlType]::Edit)
        $entry.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue('1000')
        if ($entry.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -ne '1000') { throw 'Target size did not update' }
        $before=@(Get-ChildItem -LiteralPath $ctx.work -File | ForEach-Object { $_.Name } | Sort-Object)
        Submit-MultiCompress $child
        Start-Sleep -Milliseconds 800
        Click-Element (Wait-MultiNode $child 'Cancel' ([System.Windows.Automation.ControlType]::Button) 8)
        $history=Wait-MultiHistory $child 'cancelled' 1 30
        Start-Sleep -Seconds 4
        $after=@(Get-ChildItem -LiteralPath $ctx.work -File | ForEach-Object { $_.Name } | Sort-Object)
        if (Compare-Object $before $after) { throw 'Cancellation published or removed a file' }
        Assert-MultiInputs $ctx
        # A cancelled job deliberately has no output; validate its unchanged source as
        # an image via the same output contract, and assert the absence separately.
        $validation=Assert-MultiImage $ctx $ctx.inputs[0] 2000 3000 'PNG' 'unchanged-input'
        Screenshot (Join-Path $ctx.base 'after.png')
        Save-MultiResult $ctx @{input_sha256=$ctx.hashes; before_files=$before; after_files=$after; history=$history; validator=$validation} @('after.png')
    } finally { Stop-MultiApp $child }
}
function Invoke-NegativeOverwriteScenario {
    $ctx=New-MultiContext 'negative-overwrite' @('images/tiny-8x8.png'); $child=$null
    try {
        $child=Start-MultiApp $ctx $ctx.inputs
        $null=Wait-MultiNode $child 'Compress' ([System.Windows.Automation.ControlType]::Button) 60
        Open-MultiActivity $child
        Open-MultiCompress $child; Submit-MultiCompress $child
        $null=Wait-MultiHistory $child 'done' 1
        $first=Join-Path $ctx.work 'tiny-8x8-compress.png'
        if (-not (Test-Path -LiteralPath $first -PathType Leaf)) { throw 'First plain-name output missing' }
        $firstHash=(Get-FileHash -Algorithm SHA256 -LiteralPath $first).Hash
        $v1=Assert-MultiImage $ctx $first 8 8 'PNG' 'first'
        Open-MultiCompress $child; Submit-MultiCompress $child
        $history=Wait-MultiHistory $child 'done' 2
        $second=Join-Path $ctx.work 'tiny-8x8-compress (2).png'
        if (-not (Test-Path -LiteralPath $second -PathType Leaf)) { throw 'Second suffixed output missing' }
        $secondHash=(Get-FileHash -Algorithm SHA256 -LiteralPath $second).Hash
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $first).Hash -ne $firstHash) { throw 'First output was overwritten' }
        if (@(Get-ChildItem -LiteralPath $ctx.work -File -Filter 'tiny-8x8-compress*.png').Count -ne 2) { throw 'Expected exactly two compressed outputs' }
        $v2=Assert-MultiImage $ctx $second 8 8 'PNG' 'second'; Assert-MultiInputs $ctx
        Screenshot (Join-Path $ctx.base 'after.png')
        Save-MultiResult $ctx @{input_sha256=$ctx.hashes; output_sha256=@($firstHash,$secondHash); output_names=@([IO.Path]::GetFileName($first),[IO.Path]::GetFileName($second)); history=$history; validators=@($v1,$v2)} @('after.png')
    } finally { Stop-MultiApp $child }
}
function Invoke-PersistenceScenario {
    $ctx=New-MultiContext 'persistence' @('images/tiny-8x8.png'); $child=$null
    try {
        $child=Start-MultiApp $ctx $ctx.inputs
        $null=Wait-MultiNode $child 'Compress' ([System.Windows.Automation.ControlType]::Button) 60
        Open-MultiActivity $child; Open-MultiCompress $child; Submit-MultiCompress $child
        $null=Wait-MultiHistory $child 'done' 1
        $output=Join-Path $ctx.work 'tiny-8x8-compress.png'
        if (-not (Test-Path -LiteralPath $output -PathType Leaf)) { throw 'Persistence job output missing' }
        $validation=Assert-MultiImage $ctx $output 8 8 'PNG' 'job'
        Click-MultiButton $child 'Settings'; $null=Wait-MultiText $child 'Interface build'
        Set-MultiSelection $child 'When the output exists' 'Replace existing outputs' 2
        Click-MultiButton $child 'Save settings'; $null=Wait-MultiText $child 'Settings saved'
        Screenshot (Join-Path $ctx.base 'settings-saved.png')
        if (-not $child.CloseMainWindow()) { throw 'Could not close application for persistence restart' }
        if (-not $child.WaitForExit(30000)) { throw 'Application did not exit for persistence restart' }
        $child=Start-MultiApp $ctx @()
        $null=Wait-MultiNode $child 'Settings' ([System.Windows.Automation.ControlType]::Button) 60
        Open-MultiActivity $child
        $history=Wait-MultiHistory $child 'done' 1 30
        if (-not @($history | Where-Object { $_ -match 'tiny-8x8' -and $_ -match '(?i)\bdone\b' }).Count) { throw 'Earlier job is absent from persisted history' }
        Click-MultiButton $child 'Settings'
        $build=Wait-MultiText $child 'Interface build'; $historySetting=Wait-MultiText $child 'History rows'
        $overwrite=Get-MultiSelection $child 'When the output exists'
        if ($overwrite -ne 'Replace existing outputs') { throw 'Overwrite policy did not persist' }
        Assert-MultiInputs $ctx
        Screenshot (Join-Path $ctx.base 'settings-after-restart.png')
        Save-MultiResult $ctx @{input_sha256=$ctx.hashes; output_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $output).Hash; validator=$validation; history=$history; overwrite=$overwrite; interface_build=$build; history_setting=$historySetting} @('settings-saved.png','settings-after-restart.png')
    } finally { Stop-MultiApp $child }
}
# Insert after Snapshot, Screenshot, Find-Button and Click-Element in app-e2e-windows.ps1.
# The generated table must accompany the script at ../ui/src/fields.generated.json.
function Get-DialogNodes($Element) {
    if (-not $Element) { throw 'UIA root is inaccessible' }
    $nodes = $Element.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    return ,@($nodes)
}
function Get-DialogRoot($Process) {
    $Process.Refresh()
    if ($Process.HasExited) { throw 'Application exited during dialog scenario' }
    if (-not $Process.MainWindowHandle) { return $null }
    return [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
}
function Find-DialogNode($Root, [string]$Name, [string]$Type = '') {
    foreach ($node in (Get-DialogNodes $Root)) {
        if ($node.Current.Name -eq $Name -and (-not $Type -or $node.Current.ControlType.ProgrammaticName -eq "ControlType.$Type")) { return $node }
    }
    return $null
}
function Wait-DialogButton($Process, [string]$Name, [int]$Seconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $root = Get-DialogRoot $Process
        if ($root) { $button = Find-Button $root $Name; if ($button) { return $button } }
        Start-Sleep -Milliseconds 250
    }
    throw "UIA button unavailable: $Name"
}
function Wait-DialogTool($Process, [string]$Label, [int]$Seconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $root = Get-DialogRoot $Process
        if ($root) {
            foreach ($node in (Get-DialogNodes $root)) {
                if ($node.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $node.Current.IsEnabled -and $node.Current.Name -match ('^\d{2}\s+' + [regex]::Escape($Label) + '(?:\s|$)')) { return $node }
            }
        }
        Start-Sleep -Milliseconds 250
    }
    throw "UIA tool card unavailable: $Label"
}
function Get-DialogSheet($Root, [string]$Title) {
    # Use the raw UIA ancestry: WebView2 can omit an unnamed HTML form from the
    # control view. Never infer a sheet rectangle from the controls within it.
    $close = Find-DialogNode $Root 'Close' 'Button'
    if (-not $close) { throw "Close button inaccessible for $Title" }
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $node = $walker.GetParent($close)
    while ($node) {
        $class = $node.Current.ClassName
        $role = $node.Current.LocalizedControlType
        if ($class -match '^(?i:form)$' -or $role -match '^(?i:form)$') { return $node }
        if ($node -eq $Root) { break }
        $node = $walker.GetParent($node)
    }
    throw "Sheet form is not exposed in the UIA raw tree for $Title"
}
function Find-DialogClass($Sheet, [string]$Class) {
    foreach ($node in (Get-DialogNodes $Sheet)) {
        if ($node.Current.ClassName -match ('(?:^|\s)' + [regex]::Escape($Class) + '(?:\s|$)')) { return $node }
    }
    return $null
}
function Get-DialogLayout($Process, $Tool, $Fields) {
    $root = Get-DialogRoot $Process
    $sheet = Get-DialogSheet $root $Tool.title
    $scroller = Find-DialogClass $sheet 'modal-fields'
    $preview = Find-DialogClass $sheet 'modal-preview'
    if (-not $scroller -or -not $preview) { throw "$($Tool.label): settings and preview containers are not exposed through UIA; geometry cannot be measured" }
    $settingsBox = $scroller.Current.BoundingRectangle; $previewBox = $preview.Current.BoundingRectangle
    if ($settingsBox.IsEmpty -or $previewBox.IsEmpty -or $settingsBox.Width -le 0 -or $settingsBox.Height -le 0 -or $previewBox.Width -le 0 -or $previewBox.Height -le 0) { throw "$($Tool.label): settings or preview has no measurable rectangle" }
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $rows = @()
    foreach ($field in $Fields) {
        # Check the actual visible stacked controls, not checkboxes, hidden or
        # conditionally absent fields, or custom canvas/editor surfaces.
        if ($field.hidden -or $field.show_if -or $field.kind -notin @('select','number','range','text')) { continue }
        $type = switch ($field.kind) { 'select' {'ComboBox'} 'number' {'Edit'} 'range' {'Slider'} 'text' {'Edit'} }
        $control = Find-DialogNode $scroller $field.label $type
        if (-not $control) { throw "$($Tool.label): field $($field.label) is inaccessible" }
        $branch = $control; $row = $walker.GetParent($branch); $label = $null
        while ($row -and $row -ne $scroller) {
            $label = Find-DialogNode $row $field.label 'Text'
            if ($label) { break }
            $branch = $row; $row = $walker.GetParent($row)
        }
        if (-not $label -or -not $row -or $row -eq $scroller) { throw "$($Tool.label): cannot measure row and label for $($field.label) through UIA" }
        $r = $row.Current.BoundingRectangle; $c = $branch.Current.BoundingRectangle; $l = $label.Current.BoundingRectangle
        if ($r.IsEmpty -or $c.IsEmpty -or $l.IsEmpty -or $r.Width -le 0 -or $c.Width -le 0) { throw "$($Tool.label): empty field geometry for $($field.label)" }
        $fill = $c.Width / $r.Width; $offset = [Math]::Abs($l.Left - $c.Left)
        if ($fill -lt 0.9 -or $offset -gt 4) { throw "$($Tool.label): $($field.label) row fill=$fill, label offset=$offset" }
        $rows += @{ label = $field.label; row_width = $r.Width; control_width = $c.Width; fill = $fill; label_offset = $offset }
    }
    # Compare sibling interactive controls, not ancestor/descendant pairs or the
    # canvas marks and popovers deliberately superimposed on editor surfaces.
    $controls = @((Get-DialogNodes $scroller) | Where-Object {
        $_.Current.ControlType.ProgrammaticName -in @('ControlType.Edit','ControlType.ComboBox','ControlType.Slider','ControlType.CheckBox')
    })
    for ($i = 0; $i -lt $controls.Count; $i++) {
        $a = $controls[$i]; $ar = $a.Current.BoundingRectangle; $ap = $walker.GetParent($a)
        if ($ar.IsEmpty -or $ar.Width -le 1 -or $ar.Height -le 1 -or -not $ap) { continue }
        for ($j = $i + 1; $j -lt $controls.Count; $j++) {
            $b = $controls[$j]; $bp = $walker.GetParent($b); $br = $b.Current.BoundingRectangle
            if (-not $bp -or -not [System.Windows.Automation.AutomationElement]::Compare($ap, $bp) -or $br.IsEmpty -or $br.Width -le 1 -or $br.Height -le 1) { continue }
            $w = [Math]::Min($ar.Right,$br.Right) - [Math]::Max($ar.Left,$br.Left)
            $h = [Math]::Min($ar.Bottom,$br.Bottom) - [Math]::Max($ar.Top,$br.Top)
            if ($w -gt 1 -and $h -gt 1) { throw "$($Tool.label): overlapping sibling controls $($a.Current.Name) / $($b.Current.Name)" }
        }
    }
    return @{ rows = $rows; sibling_controls = $controls.Count; settings = @($settingsBox.X,$settingsBox.Y,$settingsBox.Width,$settingsBox.Height); preview = @($previewBox.X,$previewBox.Y,$previewBox.Width,$previewBox.Height) }
}
function Assert-DialogScroll($Process, $Tool, $Fields, $Before) {
    $sheet = Get-DialogSheet (Get-DialogRoot $Process) $Tool.title
    $scroller = Find-DialogClass $sheet 'modal-fields'
    if (-not $scroller) { throw "$($Tool.label): scroller disappeared" }
    $pattern = $null
    if (-not $scroller.TryGetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern, [ref]$pattern)) { throw "$($Tool.label): settings scrolling is inaccessible through UIA" }
    $initial = $pattern.Current.VerticalScrollPercent
    $scrollable = $pattern.Current.VerticallyScrollable
    if ($scrollable) {
        $pattern.Scroll([System.Windows.Automation.ScrollAmount]::NoAmount, [System.Windows.Automation.ScrollAmount]::LargeIncrement)
        Start-Sleep -Milliseconds 300
    }
    # Reacquire all UIA objects: rendering or scrolling can rebuild the tree.
    $root = Get-DialogRoot $Process
    $sheet = Get-DialogSheet $root $Tool.title
    $newSettings = Find-DialogClass $sheet 'modal-fields'
    $newPreview = Find-DialogClass $sheet 'modal-preview'
    if (-not $newSettings -or -not $newPreview) { throw "$($Tool.label): layout became inaccessible after scroll" }
    $settingsBox = $newSettings.Current.BoundingRectangle; $previewBox = $newPreview.Current.BoundingRectangle
    if ($settingsBox.IsEmpty -or $previewBox.IsEmpty) { throw "$($Tool.label): layout geometry disappeared after scroll" }
    $settingsDelta = [Math]::Abs($settingsBox.Top - $Before.settings[1])
    $previewDelta = [Math]::Abs($previewBox.Top - $Before.preview[1])
    if ($settingsDelta -gt 2 -or $previewDelta -gt 2) { throw "$($Tool.label): layout moved while scrolling: settings=$settingsDelta preview=$previewDelta" }
    $afterPattern = $null
    if (-not $newSettings.TryGetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern, [ref]$afterPattern)) { throw "$($Tool.label): scroller pattern disappeared" }
    $after = $afterPattern.Current.VerticalScrollPercent
    if ($scrollable -and $after -le $initial) { throw "$($Tool.label): scroll did not move the settings content" }
    if ($scrollable) { $afterPattern.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, $initial); Start-Sleep -Milliseconds 300 }
    return @{ scrollable = $scrollable; before_percent = $initial; after_percent = $after; settings_top_delta = $settingsDelta; preview_top_delta = $previewDelta }
}
function Assert-DialogDesign($Root, $Tool) {
    $title = if ($Tool.title) { [string]$Tool.title } else { [string]$Tool.label }
    $sheet = Get-DialogSheet $Root $title
    $sheetBox = $sheet.Current.BoundingRectangle; $frame = $Root.Current.BoundingRectangle
    if ($sheetBox.IsEmpty -or $frame.IsEmpty -or $sheetBox.Width -le 0 -or $sheetBox.Height -le 0 -or $frame.Width -le 0 -or $frame.Height -le 0) { throw "$($Tool.label): empty UIA geometry" }
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $dialog = $walker.GetParent($sheet)
    while ($dialog -and $dialog.Current.LocalizedControlType -notmatch '^(?i:dialog)$' -and $dialog.Current.ClassName -notmatch '^(?i:dialog)$') { $dialog = $walker.GetParent($dialog) }
    if (-not $dialog) { throw "$($Tool.label): dialog wrapper is inaccessible through UIA" }
    $dialogBox = $dialog.Current.BoundingRectangle
    if ($dialogBox.IsEmpty -or $dialogBox.Width -le 0 -or $dialogBox.Height -le 0 -or $dialogBox.Left -lt $frame.Left - 2 -or $dialogBox.Top -lt $frame.Top - 2 -or $dialogBox.Right -gt $frame.Right + 2 -or $dialogBox.Bottom -gt $frame.Bottom + 2) { throw "$($Tool.label): dialog is not inside the window" }
    if ($sheetBox.Left -lt $frame.Left + 4 -or $sheetBox.Top -lt $frame.Top + 4 -or $sheetBox.Right -gt $frame.Right - 4 -or $sheetBox.Bottom -gt $frame.Bottom - 4) { throw "$($Tool.label): sheet does not retain 4px window margin" }
    if ($sheetBox.Width -ge 0.9 * $frame.Width -and $sheetBox.Height -ge 0.9 * $frame.Height) { throw "$($Tool.label): sheet occupies the window" }
    $nodes = Get-DialogNodes $sheet
    if ($nodes.Count -lt 3) { throw "$($Tool.label): sheet UIA tree inaccessible" }
    $head = @($nodes | Where-Object { $_.Current.Name -eq $title -and $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Text })
    if (-not $head.Count) { throw "$($Tool.label): expected sheet title is not exposed: $title" }
    $buttons = @($nodes | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button } | ForEach-Object { $_.Current.Name })
    $action = ([string]$Tool.action).Replace('{n}', '0').Replace('Frame(s)', 'Frames')
    if (-not $action -or 'Reset' -notin $buttons -or 'Close' -notin $buttons -or 'Cancel' -in $buttons -or ($action -ne 'Run' -and 'Run' -in $buttons)) { throw "$($Tool.label): invalid dialog footer: $($buttons -join ', ')" }
    $primary = @($buttons | Where-Object { $_ -notin @('Reset', 'Close', 'Cancel') })
    if (-not $primary.Count -or $primary[-1] -ne $action) { throw "$($Tool.label): expected primary action $action; got $($buttons -join ', ')" }
    foreach ($node in $nodes) {
        $r = $node.Current.BoundingRectangle
        if (-not $r.IsEmpty -and $r.Width -gt 1 -and ($r.Left -lt $sheetBox.Left - 2 -or $r.Right -gt $sheetBox.Right + 2)) { throw "$($Tool.label): horizontally clipped UIA element $($node.Current.Name)" }
    }
    return @{ title = $title; sheet = @($sheetBox.X,$sheetBox.Y,$sheetBox.Width,$sheetBox.Height); frame = @($frame.X,$frame.Y,$frame.Width,$frame.Height); buttons = $buttons; primary = $action; descendants = $nodes.Count }
}
function Invoke-DialogScenario([string]$Category, [string]$Relative, [string]$Target, [string]$Extension, [hashtable]$Expected) {
    $id = 'dialogs-' + $Category; $directory = Join-Path $Out $id
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $source = Join-Path $Fixtures $Relative
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixtures 'input-hashes.json') | ConvertFrom-Json
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash
    if (-not $manifest.$Relative -or $manifest.$Relative -ne $digest) { throw "Fixture digest mismatch: $Relative" }
    $inputPath = Join-Path $directory (Split-Path -Leaf $source)
    Copy-Item -LiteralPath $source -Destination $inputPath -Force
    $profileDir = Join-Path $directory 'profile'
    New-Item -ItemType Directory -Force -Path (Join-Path $profileDir 'roaming'), (Join-Path $profileDir 'local') | Out-Null
    $savedPath = $env:PATH; $savedTools = $env:CONVERTME_TOOLS_DIR; $savedAppData = $env:APPDATA; $savedLocal = $env:LOCALAPPDATA
    $child = $null
    try {
        $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $env:APPDATA = Join-Path $profileDir 'roaming'; $env:LOCALAPPDATA = Join-Path $profileDir 'local'
        $probe = & $Binary probe --tools
        if ($LASTEXITCODE -ne 0) { throw 'Tool probe failed' }
        $catalog = ($probe | Out-String) | ConvertFrom-Json
        $tools = @($catalog.tools | Where-Object { $_.category -eq $Category })
        if (-not $tools.Count) { throw "Tool probe returned no $Category tools" }
        $tablePath = Join-Path $PSScriptRoot '../ui/src/fields.generated.json'
        $tables = Get-Content -Raw -Encoding UTF8 -LiteralPath $tablePath | ConvertFrom-Json
        $categoryTable = $tables.$Category
        if (-not $categoryTable) { throw "Generated field table lacks $Category" }
        $child = Start-Process -FilePath $Binary -ArgumentList ('"' + $inputPath + '"') -WorkingDirectory $directory -PassThru
        $env:PATH = $savedPath
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools }
        $first = $tools[0]
        [void](Wait-DialogTool $child $first.label 60)
        $root = Get-DialogRoot $child
        if ((Get-DialogNodes $root).Count -lt 3) { throw 'WebView2 UIA tree is inaccessible' }
        $measurements = @{ input_sha256 = $digest; tools = @($tools | ForEach-Object { $_.label }); dialogs = @{}; fieldless = @() }
        $evidence = [System.Collections.Generic.List[object]]::new()
        $index = 0
        foreach ($tool in $tools) {
            $property = $categoryTable.PSObject.Properties[[string]$tool.id]
            if (-not $property) { throw "Generated field table lacks $Category/$($tool.id)" }
            $fields = @($property.Value)
            if (-not $fields.Count) { $measurements.fieldless += $tool.label; continue }
            $button = Wait-DialogTool $child $tool.label
            Click-Element $button
            [void](Wait-DialogButton $child 'Close')
            Start-Sleep -Seconds 2
            $root = Get-DialogRoot $child
            $design = Assert-DialogDesign $root $tool
            $sheet = Get-DialogSheet $root $tool.title
            $design.fields = @($fields | ForEach-Object { @{ key = $_.key; kind = $_.kind; label = $_.label; hidden = $_.hidden; show_if = $_.show_if } })
            # Basic visible fields map directly to accessible HTML controls. Specialized
            # editors (metadata, crop, timelines, etc.) expose their own controls.
            foreach ($field in $fields) {
                if ($field.hidden -or $field.show_if -or $field.kind -notin @('select','checkbox','number','range','text')) { continue }
                $type = switch ($field.kind) { 'select' {'ComboBox'} 'checkbox' {'CheckBox'} 'number' {'Edit'} 'range' {'Slider'} 'text' {'Edit'} }
                if (-not (Find-DialogNode $sheet $field.label $type)) { throw "$Category/$($tool.label): field $($field.label) is not exposed as $type" }
            }
            $design.layout = Get-DialogLayout $child $tool $fields
            $design.scroll = Assert-DialogScroll $child $tool $fields $design.layout
            $sheet = Get-DialogSheet (Get-DialogRoot $child) $tool.title
            $measurements.dialogs[$tool.label] = $design
            $shotName = ('dialog-{0:D2}.png' -f $index); $shot = Join-Path $directory $shotName
            Screenshot $shot
            $evidence.Add(@{ path = "$id/$shotName"; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $shot).Hash.ToLowerInvariant() })
            $before = @(Get-ChildItem -LiteralPath $directory -File | ForEach-Object { $_.Name } | Sort-Object)
            Click-Element (Find-Button $sheet 'Close')
            Start-Sleep -Milliseconds 500
            $after = @(Get-ChildItem -LiteralPath $directory -File | ForEach-Object { $_.Name } | Sort-Object)
            if (Compare-Object $before $after) { throw "$Category/$($tool.label): closing published a file" }
            $index++
        }
        # Produce and independently validate a real output after surveying the dialogs.
        if ($Category -eq 'pdf') {
            Click-Element (Wait-DialogTool $child 'Organize pages')
            Click-Element (Wait-DialogButton $child 'Save Organized PDF')
        } else { Click-Element (Wait-DialogButton $child $Target) }
        $output = $null; $deadline = [DateTime]::UtcNow.AddSeconds(240)
        while ([DateTime]::UtcNow -lt $deadline) {
            [void](Get-DialogRoot $child)
            $output = Get-ChildItem -LiteralPath $directory -File -Filter "*.$Extension" | Where-Object { $_.FullName -ne $inputPath } | Select-Object -First 1
            if ($output) { Start-Sleep -Seconds 2; break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $output) { throw "$id did not publish an output" }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $inputPath).Hash -ne $digest) { throw "$id changed its source" }
        if ($Category -eq 'images') {
            $image = [Drawing.Image]::FromFile($inputPath)
            try { $Expected.width = $image.Width; $Expected.height = $image.Height } finally { $image.Dispose() }
        }
        $kind = if ($Category -eq 'images') { 'image' } elseif ($Category -eq 'pdf') { 'pdf' } else { 'media' }
        $requestPath = Join-Path $directory 'assertion.json'
        @{ kind = $kind; path = $output.FullName; source = $inputPath; binary = $Binary; expected = $Expected } | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 -LiteralPath $requestPath
        $raw = & python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $requestPath
        if ($LASTEXITCODE -ne 0) { throw "$id validation failed: $($raw | Out-String)" }
        $measurements.validator = ($raw | Out-String) | ConvertFrom-Json
        $measurements.output_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $output.FullName).Hash
        if (-not $evidence.Count) { throw "$id has no dialog evidence" }
        $results.Add(@{ id = $id; name = $id; status = 'passed'; measurements = $measurements; evidence_files = @($evidence) })
    } finally {
        if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue; $child.WaitForExit() }
        $env:PATH = $savedPath; $env:APPDATA = $savedAppData; $env:LOCALAPPDATA = $savedLocal
        if ($null -ne $savedTools) { $env:CONVERTME_TOOLS_DIR = $savedTools } else { Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue }
    }
}
function Invoke-DialogsImages { Invoke-DialogScenario 'images' 'images/poster-portrait.png' 'JPG' 'jpg' @{ format = 'JPEG' } }
function Invoke-DialogsVideo { Invoke-DialogScenario 'video' 'video/clip-landscape-720p-10s.mp4' 'MKV' 'mkv' @{ format = @{ contains = 'matroska' }; duration = @{ min = 9; max = 11 } } }
function Invoke-DialogsAudio { Invoke-DialogScenario 'audio' 'audio/speech.mp3' 'OGG' 'ogg' @{ format = @{ contains = 'ogg' } } }
function Invoke-DialogsPdf { Invoke-DialogScenario 'pdf' 'pdf/report-5-pages.pdf' '' 'pdf' @{ pages = 5 } }
$proc = $null
try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $metadata.account = @{ sid = $identity.User.Value; elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator); session = (Get-Process -Id $PID).SessionId }
    $osInfo = Get-CimInstance Win32_OperatingSystem
    $nativeArch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $metadata.os = @{ edition = $osInfo.Caption; build = $osInfo.BuildNumber; native_arch = $nativeArch }
    foreach ($screen in [Windows.Forms.Screen]::AllScreens) {
        $metadata.monitors += @{ device = $screen.DeviceName; x = $screen.Bounds.X; y = $screen.Bounds.Y; width = $screen.Bounds.Width; height = $screen.Bounds.Height; primary = $screen.Primary }
    }
    $manifestFile = Join-Path $Fixtures 'input-hashes.json'
    if (Test-Path -LiteralPath $manifestFile -PathType Leaf) { $metadata.fixtures_manifest_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifestFile).Hash }
    if ($Suite -eq 'Full' -and $Only) { throw 'Full acceptance cannot use a scenario selector' }
    if ($Only -and $Only -ne 'convert-image') { throw "No implemented scenario matches selector: $Only" }
    if ($Suite -eq 'Full') {
        foreach ($relative in $fullFixtures) {
            if (-not (Test-Path -LiteralPath (Join-Path $Fixtures $relative) -PathType Leaf)) { throw "Missing Full fixture: $relative" }
        }
    }
    $metadata.pe_machine = Get-PeMachine $Binary
    foreach ($key in @('HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')) {
        $item = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
        if ($item) { $metadata.webview2 += $item.pv }
    }
    $manifestPath = Join-Path (Split-Path $Binary) 'tools/manifest.json'
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json
    foreach ($entry in $manifest.executables.PSObject.Properties) {
        $helper = [IO.Path]::GetFullPath((Join-Path (Split-Path $manifestPath) $entry.Value))
        $root = [IO.Path]::GetFullPath((Split-Path $manifestPath)) + [IO.Path]::DirectorySeparatorChar
        if (-not $helper.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "Escaping helper path: $helper" }
        $metadata.helpers += @{ key = $entry.Name; path = $helper; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $helper).Hash; pe_machine = Get-PeMachine $helper }
    }
    $oldPath = $env:PATH
    $oldTools = $env:CONVERTME_TOOLS_DIR
    try {
        $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
        Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
        $metadata.app_probe = (& $Binary probe 2>&1 | Out-String)
        $probeExit = $LASTEXITCODE
    } finally {
        $env:PATH = $oldPath
        if ($null -ne $oldTools) { $env:CONVERTME_TOOLS_DIR = $oldTools }
    }
    if ($probeExit -ne 0) { throw 'Packaged probe failed' }
    if (-not $Only -or $Only -eq 'convert-image') {
        if ($Suite -eq 'Full') {
            $fixture = Join-Path $Fixtures 'images/poster-portrait.png'
            $manifestFile = Join-Path $Fixtures 'input-hashes.json'
            if (-not (Test-Path -LiteralPath $manifestFile -PathType Leaf)) { throw 'Full fixture manifest is missing' }
            $fixtureManifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestFile | ConvertFrom-Json
            $expected = $fixtureManifest.'images/poster-portrait.png'
            if (-not $expected -or (Get-FileHash -Algorithm SHA256 -LiteralPath $fixture).Hash -ne $expected) { throw 'Full image fixture hash mismatch' }
            Copy-Item -LiteralPath $fixture -Destination $inputFile
        } else {
            $bitmap = New-Object System.Drawing.Bitmap(32, 24)
            try {
                for ($y = 0; $y -lt 24; $y++) {
                    for ($x = 0; $x -lt 32; $x++) {
                        $colour = if ($x -lt 16) { [Drawing.Color]::FromArgb(220, 30, 20) } else { [Drawing.Color]::FromArgb(20, 40, 220) }
                        $bitmap.SetPixel($x, $y, $colour)
                    }
                }
                $bitmap.Save($inputFile, [Drawing.Imaging.ImageFormat]::Png)
            } finally { $bitmap.Dispose() }
        }
        $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $inputFile).Hash
        $oldPath = $env:PATH
        $oldTools = $env:CONVERTME_TOOLS_DIR
        try {
            $env:PATH = (Join-Path $env:SystemRoot 'System32') + ';' + $env:SystemRoot
            Remove-Item Env:CONVERTME_TOOLS_DIR -ErrorAction SilentlyContinue
            $proc = Start-Process -FilePath $Binary -ArgumentList ('"' + $inputFile + '"') -WorkingDirectory $work -PassThru
        } finally {
            $env:PATH = $oldPath
            if ($null -ne $oldTools) { $env:CONVERTME_TOOLS_DIR = $oldTools }
        }
        $root = $null; $button = $null; $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            $proc.Refresh()
            if ($proc.HasExited) { throw "Application exited before window became ready: $($proc.ExitCode)" }
            if ($proc.MainWindowHandle -ne 0) {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
                if ($root) { $button = Find-Button $root 'JPG'; if ($button) { break } }
            }
            Start-Sleep -Milliseconds 250
        }
        if (-not $root -or -not $button) { throw 'WebView2 UIA tree or JPG conversion button unavailable' }
        $tree = Snapshot $root
        if ($tree.Count -lt 3) { throw 'WebView2 UIA tree is empty' }
        $tree | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $Out 'uia-before.json')
        Screenshot (Join-Path $Out 'before.png')
        Click-Element $button
        $output = $null; $deadline = [DateTime]::UtcNow.AddSeconds(240)
        $settings = Find-Button $root 'Settings'
        if (-not $settings) { throw 'Settings button absent from UIA tree' }
        Click-Element $settings
        Start-Sleep -Seconds 1
        $settingsTree = Snapshot $root
        $build = @($settingsTree | Where-Object { $_.name -match 'Interface build ' } | Select-Object -First 1)
        if (-not $build.Count) { throw 'Settings did not expose interface build via UIA' }
        $metadata.ui_build = $build[0].name
        $settingsTree | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $Out 'uia-settings.json')
        if ($Suite -eq 'Full') {
            $settingsPath = Join-Path $Out 'uia-settings.json'
            $results.Add(@{ id = 'build-identity'; name = 'build-identity'; status = 'passed'; measurements = @{ ui_build = $metadata.ui_build }; evidence_files = @(@{ path = 'uia-settings.json'; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $settingsPath).Hash.ToLowerInvariant() }) })
        }
        Screenshot (Join-Path $Out 'settings.png')
        while ([DateTime]::UtcNow -lt $deadline) {
            $output = Get-ChildItem -LiteralPath $work -File -Filter '*.jpg' | Select-Object -First 1
            $proc.Refresh()
            if ($proc.HasExited) { throw "Application exited during conversion: $($proc.ExitCode)" }
            if ($output) { Start-Sleep -Seconds 2; break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $output) { throw 'No JPG was published by the GUI' }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $inputFile).Hash -ne $sourceHash) { throw 'Input changed during conversion' }
        $image = [Drawing.Image]::FromFile($inputFile)
        try { $width = $image.Width; $height = $image.Height } finally { $image.Dispose() }
        $firstX = [int][Math]::Floor($width / 4)
        $secondX = [int][Math]::Floor(3 * $width / 4)
        $middleY = [int][Math]::Floor($height / 2)
        $request = @{
            kind = 'image'; path = $output.FullName; source = $inputFile; binary = $Binary
            expected = @{
                format = 'JPEG'; width = $width; height = $height
                source_pixels = @(
                    @{ x = $firstX; y = $middleY; source_x = $firstX; source_y = $middleY; tolerance = 80 },
                    @{ x = $secondX; y = $middleY; source_x = $secondX; source_y = $middleY; tolerance = 80 }
                )
            }
        }
        $requestPath = Join-Path $work 'image-assertion.json'
        $request | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 -LiteralPath $requestPath
        $facts = & python (Join-Path $PSScriptRoot 'acceptance-contract.py') assert-output --request $requestPath
        if ($LASTEXITCODE -ne 0) { throw "Image validator failed with exit code $LASTEXITCODE`: $($facts | Out-String)" }
        Screenshot (Join-Path $Out 'after.png')
        $results.Add(@{ id = 'convert-image'; name = 'convert-image'; status = 'passed'; measurements = @{ input_sha256 = $sourceHash; output_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $output.FullName).Hash; validator = ($facts | Out-String) }; output = $output.FullName; evidence_files = @(@{ path = 'after.png'; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Out 'after.png')).Hash.ToLowerInvariant() }) })
    }
    if ($Suite -eq 'Full') {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force; $proc.WaitForExit() }
        foreach ($scenario in @(
            @{ id = 'convert-video'; source = 'video/clip-landscape-720p-10s.mp4'; target = 'MKV'; extension = 'mkv'; expected = @{ format = @{ contains = 'matroska' }; duration = @{ min = 9; max = 11 } } },
            @{ id = 'convert-audio'; source = 'audio/speech.mp3'; target = 'OGG'; extension = 'ogg'; expected = @{ format = @{ contains = 'ogg' } } }
        )) {
            try { Invoke-MediaScenario $scenario.id $scenario.source $scenario.target $scenario.extension $scenario.expected }
            catch { $results.Add(@{ id = $scenario.id; name = $scenario.id; status = 'failed'; reason = $_.Exception.Message; measurements = @{}; evidence_files = @() }) }
        }
        try { Invoke-CompressScale }
        catch { $results.Add(@{ id = 'compress-scale'; name = 'compress-scale'; status = 'failed'; reason = $_.Exception.Message; measurements = @{}; evidence_files = @() }) }
        try { Invoke-PdfOrganize }
        catch { $results.Add(@{ id = 'pdf-organize'; name = 'pdf-organize'; status = 'failed'; reason = $_.Exception.Message; measurements = @{}; evidence_files = @() }) }
        foreach ($scenario in @(
            @{id='multi-file'; action={ Invoke-MultiFileScenario $false }},
            @{id='multi-file-per-file'; action={ Invoke-MultiFileScenario $true }},
            @{id='negative-cancel'; action={ Invoke-NegativeCancelScenario }},
            @{id='negative-overwrite'; action={ Invoke-NegativeOverwriteScenario }},
            @{id='persistence'; action={ Invoke-PersistenceScenario }}
        )) {
            try { & $scenario.action }
            catch { $results.Add(@{id=$scenario.id; name=$scenario.id; status='failed'; reason=$_.Exception.Message; measurements=@{}; evidence_files=@()}) }
        }
        foreach ($scenario in @(
            @{ id = 'dialogs-images'; run = { Invoke-DialogsImages } },
            @{ id = 'dialogs-video'; run = { Invoke-DialogsVideo } },
            @{ id = 'dialogs-audio'; run = { Invoke-DialogsAudio } },
            @{ id = 'dialogs-pdf'; run = { Invoke-DialogsPdf } }
        )) {
            try { & $scenario.run }
            catch { $results.Add(@{ id = $scenario.id; name = $scenario.id; status = 'failed'; reason = $_.Exception.Message; measurements = @{}; evidence_files = @() }) }
        }
    }
} catch {
    $results.Add(@{ name = 'desktop'; status = 'failed'; reason = $_.Exception.Message })
} finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    $env:APPDATA = $oldAppData
    $env:LOCALAPPDATA = $oldLocalAppData
    if ($Suite -eq 'Full') {
        foreach ($id in $fullScenarioIds) {
            if (-not @($results | Where-Object { $_.id -eq $id }).Count) {
                $results.Add(@{ id = $id; name = $id; status = 'blocked'; reason = 'Scenario was not executed by this harness'; measurements = @{}; evidence_files = @() })
            }
        }
    }
    $automated = $Suite -eq 'Full' -and @($results | Where-Object { $_.status -ne 'passed' }).Count -eq 0 -and @($results).Count -eq $fullScenarioIds.Count
    $report = @{ metadata = $metadata; suite = $Suite; selectors = @{ only = $Only }; results = @($results); full_automated_acceptance = $automated; full_desktop_acceptance = $false; unrun = @($results | Where-Object { $_.status -eq 'blocked' } | ForEach-Object { $_.id }) }
    $report | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 (Join-Path $Out 'report.json')
}
if (@($results | Where-Object { $_.name -eq 'convert-image' -and $_.status -eq 'passed' }).Count -ne 1 -or @($results | Where-Object { $_.status -in @('failed', 'blocked') }).Count -gt 0) { exit 1 }
