<#
.SYNOPSIS
  Timed, unattended xLODGen run through MO2 that produces a golden output set plus timing data.

.DESCRIPTION
  xLODGen has no auto-generate switch: its options live in Plugins.sseviewsettings
  ([SSE LOD Options]) and generation starts from the TfrmLODGen dialog. This script
    1. backs up Plugins.sseviewsettings and applies a preset to [SSE LOD Options],
    2. launches the MO2 executable entry through `ModOrganizer.exe run` (so the VFS is active),
       redirecting output with -o:,
    3. clicks Generate in the dialog via UI Automation (the dialog's default worldspace is Tamriel),
    4. samples the main window caption (stage + elapsed), CPU, memory and LODGen child processes,
    5. closes xLODGen once generation is done, copies its logs and writes summary.json,
    6. restores Plugins.sseviewsettings.

.EXAMPLE
  pwsh -File Run-XLodGenGolden.ps1 -Preset no-seasons
  pwsh -File Run-XLodGenGolden.ps1 -Preset seasons
#>
[CmdletBinding()]
param(
    [ValidateSet('no-seasons', 'seasons', 'trees-only')] [string] $Preset = 'no-seasons',
    [string] $Instance = 'D:\Modlists\skyrim-cs',
    [string] $Mo2Profile = 'Default',
    [string] $Executable = 'xLODGen x64',
    [string] $OutRoot = 'D:\Modlists\LODForge-golden',
    [string] $SettingsFile = "$env:LOCALAPPDATA\Skyrim Special Edition\Plugins.sseviewsettings",
    [int] $TimeoutMinutes = 600
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]
$Tree = [Windows.Automation.TreeScope]

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$runDir = Join-Path $OutRoot "$Preset-$stamp"
$outDir = Join-Path $runDir 'output'
New-Item -ItemType Directory -Force $outDir | Out-Null
$timeline = Join-Path $runDir 'timeline.csv'
'utc,elapsed_s,stage,cpu_s,working_set_mb,lodgen_procs' | Set-Content $timeline

function Write-Step([string] $msg) { Write-Host "[$(Get-Date -Format HH:mm:ss)] $msg" }

# Sets key=value lines inside one INI section, appending keys the section lacks.
function Set-IniSection([string] $path, [string] $section, [hashtable] $values) {
    $lines = [Collections.Generic.List[string]](Get-Content $path)
    $start = $lines.IndexOf("[$section]")
    if ($start -lt 0) { $lines.Add(''); $lines.Add("[$section]"); $start = $lines.Count - 1 }
    $end = $start + 1
    while ($end -lt $lines.Count -and $lines[$end] -notmatch '^\[') { $end++ }
    $pending = @{} + $values
    for ($i = $start + 1; $i -lt $end; $i++) {
        if ($lines[$i] -match '^([^=;]+)=') {
            $k = $Matches[1]
            if ($pending.ContainsKey($k)) { $lines[$i] = "$k=$($pending[$k])"; $pending.Remove($k) }
        }
    }
    $insertAt = $end
    foreach ($k in $pending.Keys) { $lines.Insert($insertAt, "$k=$($pending[$k])"); $insertAt++ }
    Set-Content $path $lines
}

function Read-Preset([string] $path) {
    $h = [ordered]@{}
    foreach ($l in Get-Content $path) {
        if ($l -match '^\s*([^;=\s][^=]*)=(.*)$') { $h[$Matches[1].Trim()] = $Matches[2].Trim() }
    }
    $h
}

# xLODGen is Delphi VCL: its controls expose no UIA patterns, so UIA is only used to find
# window handles and the rest goes through Win32 messages.
Add-Type -Namespace LodForge -Name Win32 -MemberDefinition '
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessage(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessage(IntPtr hWnd, int msg, IntPtr wParam, System.Text.StringBuilder lParam);
[DllImport("user32.dll")]
public static extern bool PostMessage(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);
'
$BM_GETCHECK = 0x00F0; $BM_CLICK = 0x00F5; $LB_GETTEXT = 0x0189; $LB_GETCOUNT = 0x018B

# The LOD options dialog is owned by the main form, so UIA lists it under TfrmMain, not the desktop.
function Find-LodDialog([int] $procId) {
    $main = $UIA::RootElement.FindFirst($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $procId)))
    if (-not $main) { return $null }
    $main.FindFirst($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'TfrmLODGen')))
}

function Get-Hwnd($element) { [IntPtr]$element.Current.NativeWindowHandle }

function Find-Control($dialog, [string] $className, [string] $name) {
    $dialog.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $className))) |
        Where-Object { $_.Current.Name -eq $name } | Select-Object -First 1
}

foreach ($n in 'xLODGenx64', 'xLODGen', 'LODGenx64') {
    if (Get-Process $n -ErrorAction SilentlyContinue) { throw "$n is already running; close it first." }
}

$mo2 = Join-Path $Instance 'ModOrganizer.exe'
$toolDir = Join-Path $Instance 'tools\xLODGen'
$presetValues = Read-Preset (Join-Path $PSScriptRoot "presets\$Preset.ini")
$backup = "$SettingsFile.lodforge-backup-$stamp"
Copy-Item $SettingsFile $backup
Write-Step "Settings backed up to $backup"

$summary = [ordered]@{
    preset = $Preset; instance = $Instance; profile = $Mo2Profile; started_utc = (Get-Date).ToUniversalTime().ToString('o')
    machine = [ordered]@{
        cpu = (Get-CimInstance Win32_Processor).Name
        logical_cpus = [Environment]::ProcessorCount
        ram_gb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
    }
    preset_values = $presetValues
}
$proc = $null
try {
    Set-IniSection $SettingsFile 'SSE LOD Options' $presetValues

    # One argv string for MO2; inner quotes escaped as \" (no trailing backslashes before them).
    if ($outDir -match ' ') { throw "OutRoot must not contain spaces: $outDir" }
    $toolArgs = "-sse -d:\`"$Instance\Stock Game\Data\`" -o:$outDir"
    Write-Step "Launching '$Executable' via MO2 profile '$Mo2Profile' with: $toolArgs"
    Start-Process $mo2 -ArgumentList "-p `"$Mo2Profile`" run -e -a `"$toolArgs`" `"$Executable`""

    $deadline = (Get-Date).AddMinutes(5)
    while (-not ($proc = Get-Process xLODGenx64 -ErrorAction SilentlyContinue | Select-Object -First 1)) {
        if ((Get-Date) -gt $deadline) { throw 'xLODGenx64 did not start within 5 minutes.' }
        Start-Sleep -Milliseconds 500
    }
    $launched = Get-Date
    Write-Step "xLODGen PID $($proc.Id); waiting for LOD options dialog (plugin load)"

    $dialog = $null
    $deadline = (Get-Date).AddMinutes(30)
    while (-not $dialog) {
        if ($proc.HasExited) { throw 'xLODGen exited before showing the LOD dialog.' }
        if ((Get-Date) -gt $deadline) { throw 'LOD dialog did not appear within 30 minutes.' }
        Start-Sleep -Seconds 1
        $dialog = Find-LodDialog $proc.Id
    }
    $dialogShown = Get-Date
    $summary.load_seconds = [math]::Round(($dialogShown - $launched).TotalSeconds, 1)
    Write-Step "Dialog up after $($summary.load_seconds)s"

    # Record the worldspace list (xLODGen pre-checks Tamriel only) and the option checkboxes as the dialog shows them.
    $list = $dialog.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'TCheckListBox')))
    $worldspaces = @()
    if ($list) {
        $h = Get-Hwnd $list
        $n = [int][LodForge.Win32]::SendMessage($h, $LB_GETCOUNT, [IntPtr]::Zero, [IntPtr]::Zero)
        for ($i = 0; $i -lt $n; $i++) {
            $sb = New-Object Text.StringBuilder 512
            $null = [LodForge.Win32]::SendMessage($h, $LB_GETTEXT, [IntPtr]$i, $sb)
            $worldspaces += $sb.ToString()
        }
    }
    $summary.worldspace_list = $worldspaces
    $options = [ordered]@{}
    foreach ($name in 'Objects LOD', 'Trees LOD', 'Terrain LOD', 'Build meshes', 'Build diffuse', 'Build normal', 'Seasons', 'Snow', 'Build atlas', 'Height maps', 'Occlusion') {
        $cb = Find-Control $dialog 'TCheckBox' $name
        if ($cb) { $options[$name] = [int][LodForge.Win32]::SendMessage((Get-Hwnd $cb), $BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero) -eq 1 }
    }
    $summary.dialog_options = $options
    Write-Step "Dialog options: $(($options.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', ')"

    $generate = Find-Control $dialog 'TButton' 'Generate'
    if (-not $generate) { throw 'Generate button not found.' }
    # Posted, not sent: the click closes a modal dialog and generation then runs on xLODGen's UI thread.
    $null = [LodForge.Win32]::PostMessage((Get-Hwnd $generate), $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
    $genStart = Get-Date
    Write-Step "Generate clicked; sampling every 2s"

    $lastStage = ''; $seenGenerating = $false; $idleSince = $null
    $peakWs = 0; $peakLodgen = 0
    $deadline = $genStart.AddMinutes($TimeoutMinutes)
    while ($true) {
        if ($proc.HasExited) { Write-Step 'xLODGen exited on its own'; break }
        if ((Get-Date) -gt $deadline) { throw "Generation exceeded $TimeoutMinutes minutes." }
        Start-Sleep -Seconds 2
        $proc.Refresh()
        $title = $proc.MainWindowTitle
        $stage = ($title -split ', Elapsed Time:')[0]
        $lodgen = @(Get-Process LODGenx64, LODGen -ErrorAction SilentlyContinue).Count
        $ws = [math]::Round($proc.WorkingSet64 / 1MB)
        if ($ws -gt $peakWs) { $peakWs = $ws }
        if ($lodgen -gt $peakLodgen) { $peakLodgen = $lodgen }
        $el = [math]::Round(((Get-Date) - $genStart).TotalSeconds, 1)
        '{0},{1},"{2}",{3},{4},{5}' -f (Get-Date).ToUniversalTime().ToString('o'), $el, ($stage -replace '"', "'"), [math]::Round($proc.TotalProcessorTime.TotalSeconds, 1), $ws, $lodgen | Add-Content $timeline
        if ($stage -ne $lastStage) { Write-Step "[$el s] $stage"; $lastStage = $stage }

        if ($title -match 'Generating LOD') { $seenGenerating = $true; $idleSince = $null }
        elseif ($seenGenerating -and $lodgen -eq 0) {
            if (-not $idleSince) { $idleSince = Get-Date }
            elseif (((Get-Date) - $idleSince).TotalSeconds -ge 6) { break }
        }
    }
    $genEnd = if ($idleSince) { $idleSince } else { Get-Date }
    $summary.generate_seconds = [math]::Round(($genEnd - $genStart).TotalSeconds, 1)
    $summary.total_seconds = [math]::Round(($genEnd - $launched).TotalSeconds, 1)
    $summary.xlodgen_cpu_seconds = [math]::Round($proc.TotalProcessorTime.TotalSeconds, 1)
    $summary.peak_working_set_mb = $peakWs
    $summary.peak_concurrent_lodgen = $peakLodgen
    Write-Step "Generation done in $($summary.generate_seconds)s; closing xLODGen"

    if (-not $proc.HasExited) {
        $null = $proc.CloseMainWindow()
        if (-not $proc.WaitForExit(120000)) { Write-Step 'Close timed out; killing'; $proc.Kill() }
    }
}
finally {
    Copy-Item $backup $SettingsFile -Force
    Write-Step 'Settings restored'
}

# Logs: this session's part of SSELODGen_log.txt, plus LODGen logs written during the run.
$log = Join-Path $toolDir 'SSELODGen_log.txt'
if (Test-Path $log) {
    $lines = Get-Content $log
    $startIdx = 0
    for ($i = $lines.Count - 1; $i -ge 0; $i--) { if ($lines[$i] -match 'starting session') { $startIdx = $i; break } }
    $session = $lines[$startIdx..($lines.Count - 1)]
    $session | Set-Content (Join-Path $runDir 'SSELODGen_log.txt')
    $summary.stage_elapsed = @($session | Select-String '\[(\d+:\d+)\] (.+?) elapsed time: (\d+:\d+)' | ForEach-Object {
        [ordered]@{ at = $_.Matches[0].Groups[1].Value; stage = $_.Matches[0].Groups[2].Value; elapsed = $_.Matches[0].Groups[3].Value }
    })
    $summary.worldspace_starts = @($session | Select-String '\[(\d+:\d+)\] \[(\w+)\] (Generating .+)$' | ForEach-Object {
        [ordered]@{ at = $_.Matches[0].Groups[1].Value; worldspace = $_.Matches[0].Groups[2].Value; what = $_.Matches[0].Groups[3].Value }
    })
}
Get-ChildItem $toolDir, (Join-Path $toolDir 'Edit Scripts') -Filter 'LODGen*log*.txt' -ErrorAction SilentlyContinue |
    Where-Object LastWriteTime -gt $launched | Copy-Item -Destination $runDir

$files = Get-ChildItem $outDir -Recurse -File
$summary.output = [ordered]@{
    files = $files.Count
    mb = [math]::Round(($files | Measure-Object Length -Sum).Sum / 1MB, 1)
    by_extension = [ordered]@{}
}
foreach ($g in $files | Group-Object { $_.Extension.ToLower() } | Sort-Object Name) {
    $summary.output.by_extension[$g.Name] = [ordered]@{ files = $g.Count; mb = [math]::Round(($g.Group | Measure-Object Length -Sum).Sum / 1MB, 1) }
}
$summary.finished_utc = (Get-Date).ToUniversalTime().ToString('o')
$summary | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $runDir 'summary.json')
Write-Step "Summary: $(Join-Path $runDir 'summary.json')"
