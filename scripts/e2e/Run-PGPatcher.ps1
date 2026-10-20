<#
.SYNOPSIS
  Runs PGPatcher (ParallaxGen) headless through MO2's VFS for one profile and installs its output as
  a mod at the top of that profile.

.DESCRIPTION
  PGPatcher reads the profile MO2 has selected in ModOrganizer.ini, so the script switches
  selected_profile for the run and restores it afterwards. Settings live in
  <PGPatcher>\cfg\settings.json (game, MO2 instance, output folder). The launch goes through WMI so
  usvfs can inject from a sandboxed shell. Refuses while MO2 or the game is running.
#>
[CmdletBinding()]
param(
    [string] $Instance = 'D:\Modlists\skyrim-cs',
    [string] $Mo2Profile = 'LODForge-E2E',
    [string] $PGPatcher = 'D:\Modlists\skyrim-cs\tools\PGPatcher\PGPatcher.exe',
    [string] $OutputMod = 'PGPatcher Output',
    [int] $TimeoutMinutes = 60
)
$ErrorActionPreference = 'Stop'
function Write-Step([string] $m) { Write-Host "[$(Get-Date -Format HH:mm:ss)] $m" }
if (Get-Process -Name ModOrganizer, SkyrimSE, PGPatcher -ErrorAction SilentlyContinue) { throw 'MO2, the game or PGPatcher is running' }

$ini = Join-Path $Instance 'ModOrganizer.ini'
$backup = "$ini.before-pgpatcher"
Copy-Item $ini $backup -Force
$text = [IO.File]::ReadAllText($ini)
$was = [regex]::Match($text, 'selected_profile=@ByteArray\(([^)]*)\)').Groups[1].Value
[IO.File]::WriteAllText($ini, [regex]::Replace($text, 'selected_profile=@ByteArray\([^)]*\)', "selected_profile=@ByteArray($Mo2Profile)"))
Write-Step "selected_profile $was -> $Mo2Profile"
$out = Join-Path $Instance "mods\$OutputMod"
try {
    $cmd = "`"$Instance\ModOrganizer.exe`" -p `"$Mo2Profile`" run `"$PGPatcher`" -a `"--autostart`""
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd; CurrentDirectory = $Instance }
    if ($r.ReturnValue -ne 0) { throw "WMI launch failed ($($r.ReturnValue))" }
    $deadline = (Get-Date).AddMinutes(2)
    while (-not (Get-Process -Name PGPatcher -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep 2 }
    if (-not (Get-Process -Name PGPatcher -ErrorAction SilentlyContinue)) { throw 'PGPatcher did not start' }
    Write-Step 'PGPatcher running'
    $end = (Get-Date).AddMinutes($TimeoutMinutes)
    while ((Get-Process -Name PGPatcher -ErrorAction SilentlyContinue) -and (Get-Date) -lt $end) { Start-Sleep 5 }
    if (Get-Process -Name PGPatcher -ErrorAction SilentlyContinue) { throw 'PGPatcher still running at timeout' }
    $deadline = (Get-Date).AddMinutes(2)
    while ((Get-Process -Id $r.ProcessId -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep 2 }
    Write-Step 'PGPatcher exited'
}
finally {
    # Restore the user's selected profile (keep any other changes MO2 made to the ini).
    if (-not (Get-Process -Name ModOrganizer -ErrorAction SilentlyContinue)) {
        $now = [IO.File]::ReadAllText($ini)
        [IO.File]::WriteAllText($ini, [regex]::Replace($now, 'selected_profile=@ByteArray\([^)]*\)', "selected_profile=@ByteArray($was)"))
        Write-Step "selected_profile restored to $was"
    } else { Write-Step "MO2 still open: restore selected_profile=$was by hand (backup $backup)" }
}
if (Test-Path $out) {
    $files = Get-ChildItem $out -Recurse -File
    Write-Step ("output: {0} files, {1:N0} MB in {2}" -f $files.Count, (($files | Measure-Object Length -Sum).Sum / 1MB), $out)
    $modlist = Join-Path $Instance "profiles\$Mo2Profile\modlist.txt"
    $lines = [IO.File]::ReadAllLines($modlist)
    if ($lines -notcontains "+$OutputMod") {
        $lines = @($lines[0], "+$OutputMod") + @($lines | Select-Object -Skip 1 | Where-Object { $_ -ne "-$OutputMod" })
        [IO.File]::WriteAllLines($modlist, $lines)
        Write-Step "enabled '$OutputMod' at the top of $Mo2Profile"
    }
    $esps = Get-ChildItem $out -File -Filter *.esp
    foreach ($e in $esps) {
        foreach ($f in 'plugins.txt', 'loadorder.txt') {
            $p = Join-Path $Instance "profiles\$Mo2Profile\$f"; $l = [IO.File]::ReadAllLines($p)
            $entry = if ($f -eq 'plugins.txt') { "*$($e.Name)" } else { $e.Name }
            if (-not ($l | Where-Object { $_.TrimStart('*') -eq $e.Name })) { [IO.File]::WriteAllLines($p, @($l) + $entry); Write-Step "$f += $entry" }
        }
    }
} else { Write-Step 'no output folder written' }
$logs = Get-ChildItem (Split-Path $PGPatcher) -Recurse -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($logs) { Write-Step "log: $($logs.FullName)"; Get-Content $logs.FullName -Tail 15 }
