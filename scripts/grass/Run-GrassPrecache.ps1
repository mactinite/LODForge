<#
.SYNOPSIS
  Unattended NGIO grass precache for one MO2 profile, for the default season and/or forced Seasons
  of Skyrim seasons: relaunches the game until NGIO finishes each one.

.DESCRIPTION
  No Grass In Objects NG generates the cache when PrecacheGrass.txt sits next to SkyrimSE.exe (it
  crashes now and then; the next launch continues from the progress file). Grass Cache Helper NG
  loads <cell>.<SEASON>.cgid for the current Seasons of Skyrim season, but precaching always writes
  plain <cell>.cgid (with the forced season's grass in it), so with -TargetMod each season's output
  is renamed and moved into the mod before the next season. The default cache needs Seasons of
  Skyrim off:
  - Default disables the "Seasons of Skyrim SKSE" mod for the run;
  - WIN/SPR/SUM/AUT keep it enabled and force the season with `Season Type` 1/2/3/4 in
    overwrite\SKSE\Plugins\po3_SeasonsOfSkyrim.ini (restored to its old value afterwards).
  The cache lands in <instance>\overwrite\Grass (MO2 VFS); -TargetMod moves it into
  mods\<TargetMod>\Grass when done. NGIO's GrassControl.ini should have Updating-Cache = true so a
  season run keeps the other seasons' files.

  Run details handled here:
  - MO2's Root Builder moves new game-root files to overwrite\Root on exit and deploys them again,
    so the progress file can be in either place; both copies are cleared once a season is done.
  - After a crash MO2 shows a modal "Failed to run 'SKSE'" and stays open; once that line is in its
    log the script closes the MO2 it launched (its PID only, never another MO2).
  - When NGIO logs "Grass generation finished successfully!" it shows a message box and leaves the
    game running; -CloseGameWhenDone closes that game process (the one this run launched).
  - MO2 `-p <profile>` persists the profile as selected; the old selection is restored at the end.
  Launches go through WMI so usvfs can inject from a sandboxed shell.

.EXAMPLE
  pwsh -File Run-GrassPrecache.ps1 -Mo2Profile LODForge-E2E -Season Default
  pwsh -File Run-GrassPrecache.ps1 -Mo2Profile LODForge-E2E -Season WIN,SPR,SUM,AUT -CloseGameWhenDone -TargetMod 'Grass Cache - LODForge-E2E'
#>
[CmdletBinding()]
param(
    [string] $Instance = 'D:\Modlists\skyrim-cs',
    [string] $Mo2Profile = 'Default',
    # Default, WIN, SPR, SUM, AUT; comma-separated works too (pwsh -File passes "WIN,SPR" as one string).
    [string[]] $Season = @('Default'),
    [string] $SeasonsMod = 'Seasons of Skyrim SKSE',
    [int] $MaxLaunches = 40,
    [int] $TimeoutHours = 6,
    [switch] $CloseGameWhenDone,
    [string] $TargetMod
)

$ErrorActionPreference = 'Stop'
function Write-Step([string] $msg) { Write-Host "[$(Get-Date -Format HH:mm:ss)] $msg" }
$Season = @($Season | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$bad = $Season | Where-Object { $_ -notin 'Default', 'WIN', 'SPR', 'SUM', 'AUT' }
if ($bad) { throw "unknown season(s): $($bad -join ', ') (use Default, WIN, SPR, SUM, AUT)" }

$game = Join-Path $Instance 'Stock Game'
$trigger = Join-Path $game 'PrecacheGrass.txt'
$rootTrigger = Join-Path $Instance 'overwrite\Root\PrecacheGrass.txt'
function Get-Trigger { @($trigger, $rootTrigger) | Where-Object { Test-Path $_ } | Select-Object -First 1 }
function Get-Progress { $t = Get-Trigger; if ($t) { (Get-Content $t | Measure-Object -Line).Lines } else { -1 } }
function Clear-Trigger { foreach ($t in $trigger, $rootTrigger) { if (Test-Path $t) { Remove-Item -Force $t } } }
$modlist = Join-Path $Instance "profiles\$Mo2Profile\modlist.txt"
$grassDir = Join-Path $Instance 'overwrite\Grass'
$mo2Log = Join-Path $Instance 'logs\mo_interface.log'
$ngioLog = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'My Games\Skyrim Special Edition\SKSE\NGIO-NG.log'
$sosIni = Join-Path $Instance 'overwrite\SKSE\Plugins\po3_SeasonsOfSkyrim.ini'
$mo2Ini = Join-Path $Instance 'ModOrganizer.ini'
$seasonType = @{ WIN = 1; SPR = 2; SUM = 3; AUT = 4 }

if (Get-Process -Name ModOrganizer, SkyrimSE -ErrorAction SilentlyContinue) { throw 'MO2 or the game is running; close it first.' }

function Set-SeasonsMod([bool] $on) {
    $l = [IO.File]::ReadAllLines($modlist)
    [IO.File]::WriteAllLines($modlist, ($l | ForEach-Object {
        if ($_ -eq "+$SeasonsMod" -or $_ -eq "-$SeasonsMod") { $(if ($on) { "+$SeasonsMod" } else { "-$SeasonsMod" }) } else { $_ } }))
}
function Set-SeasonType([int] $v) {
    $t = [IO.File]::ReadAllText($sosIni)
    [IO.File]::WriteAllText($sosIni, [regex]::Replace($t, '(?m)^Season Type\s*=\s*\d+', "Season Type = $v"))
}

Copy-Item $modlist "$modlist.before-grass-precache" -Force
# MO2 drops the activation mark of a plugin whose mod is disabled (SnowOverSkyrim.esp ships in the
# Seasons of Skyrim mod), so the active plugins are noted here and re-marked at the end.
$pluginsTxt = Join-Path $Instance "profiles\$Mo2Profile\plugins.txt"
$activeBefore = @([IO.File]::ReadAllLines($pluginsTxt) | Where-Object { $_.StartsWith('*') } | ForEach-Object { $_.Substring(1) })
$seasonsWasOn = ([IO.File]::ReadAllLines($modlist)) -contains "+$SeasonsMod"
# The target cache mod stays disabled during the run: MO2 edits files that already exist in a mod in
# place, so a run would overwrite the mod's other-season files (seen when Grass Cache Helper fell back
# to plain names). New files go to overwrite\Grass and are moved in at the end.
$targetWasOn = $TargetMod -and (([IO.File]::ReadAllLines($modlist)) -contains "+$TargetMod")
function Set-ModEnabled([string] $name, [bool] $on) {
    $l = [IO.File]::ReadAllLines($modlist)
    [IO.File]::WriteAllLines($modlist, ($l | ForEach-Object { if ($_ -eq "+$name" -or $_ -eq "-$name") { $(if ($on) { "+$name" } else { "-$name" }) } else { $_ } }))
}
if ($targetWasOn) { Set-ModEnabled $TargetMod $false; Write-Step "disabled '$TargetMod' for the run" }
if ((Test-Path $grassDir) -and (Get-ChildItem $grassDir -File)) { throw "$grassDir is not empty; move or remove it first" }
$oldSeasonType = if (Test-Path $sosIni) { [int]([regex]::Match([IO.File]::ReadAllText($sosIni), '(?m)^Season Type\s*=\s*(\d+)').Groups[1].Value) } else { $null }
$oldSelected = [regex]::Match([IO.File]::ReadAllText($mo2Ini), 'selected_profile=@ByteArray\(([^)]*)\)').Groups[1].Value
if (($Season | Where-Object { $_ -ne 'Default' }) -and $null -eq $oldSeasonType) { throw "no Season Type in $sosIni (run the game once with Seasons of Skyrim first)" }

$start = Get-Date
$results = @()
try {
    foreach ($s in $Season) {
        if ($s -eq 'Default') { Set-SeasonsMod $false; Write-Step "[$s] Seasons of Skyrim disabled" }
        else { Set-SeasonsMod $true; Set-SeasonType $seasonType[$s]; Write-Step "[$s] Seasons of Skyrim on, Season Type = $($seasonType[$s])" }
        Clear-Trigger
        New-Item -ItemType File -Force $trigger | Out-Null
        $seasonStart = Get-Date
        $finished = $false
        $stalls = 0
        for ($n = 1; $n -le $MaxLaunches -and -not $finished; $n++) {
            if ((Get-Date) - $start -gt [TimeSpan]::FromHours($TimeoutHours)) { Write-Step 'Timeout'; break }
            $progressBefore = Get-Progress
            $launchAt = Get-Date
            Write-Step "[$s] launch $n (progress lines $progressBefore)"
            $cmd = "`"$Instance\ModOrganizer.exe`" -p `"$Mo2Profile`" run -e SKSE"
            $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd; CurrentDirectory = $Instance }
            if ($r.ReturnValue -ne 0) { throw "WMI launch failed ($($r.ReturnValue))" }
            $deadline = (Get-Date).AddMinutes(30)
            $gameProc = $null
            while (-not $gameProc -and (Get-Date) -lt $deadline) { Start-Sleep 2; $gameProc = Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -ge $launchAt.AddSeconds(-5) } | Select-Object -First 1 }
            while ($gameProc -and -not $gameProc.HasExited) {
                Start-Sleep 5
                if (-not $finished -and (Test-Path $ngioLog) -and (Get-Item $ngioLog).LastWriteTime -ge $launchAt -and (Select-String $ngioLog -Pattern 'Grass generation finished successfully' -Quiet)) {
                    $finished = $true
                    Write-Step "[$s] NGIO reports the precache finished"
                    if ($CloseGameWhenDone) { Start-Sleep 5; Write-Step "[$s] closing the game (pid $($gameProc.Id))"; Stop-Process -Id $gameProc.Id -Force -ErrorAction SilentlyContinue }
                    else { Write-Step "[$s] waiting for the game to be closed (click OK on NGIO's message, then quit)" }
                }
            }
            $mo2End = (Get-Date).AddMinutes(3)
            while ((Get-Process -Id $r.ProcessId -ErrorAction SilentlyContinue) -and (Get-Date) -lt $mo2End) {
                if ((Get-Content $mo2Log -Tail 3 -ErrorAction SilentlyContinue) -match "Failed to run 'SKSE'") {
                    Write-Step "closing MO2 (pid $($r.ProcessId)) left open after the game's exit"
                    Stop-Process -Id $r.ProcessId -Force -ErrorAction SilentlyContinue; Start-Sleep 3; break
                }
                Start-Sleep 2
            }
            if (-not $finished) {
                $progressAfter = Get-Progress
                Write-Step "[$s] game exited after launch $n; progress lines $progressBefore -> $progressAfter"
                if ($progressAfter -lt 0) { $finished = $true }   # progress file gone: NGIO finished
                elseif ($progressAfter -le $progressBefore) { $stalls++; if ($stalls -ge 2) { Write-Step "[$s] no progress in two launches; stopping"; break } }
                else { $stalls = 0 }
            }
        }
        Clear-Trigger
        # NGIO + Grass Cache Helper always write plain <cell>.cgid while precaching (the helper only
        # applies season suffixes when loading), with the forced season's grass in them. So each
        # season's output is renamed to <cell>.<SEASON>.cgid and moved out before the next season.
        $suffix = if ($s -eq 'Default') { '' } else { ".$s" }
        $new = @(Get-ChildItem $grassDir -File -Filter *.cgid -ErrorAction SilentlyContinue | Where-Object { $_.Name -notmatch '\.(WIN|SPR|SUM|AUT)\.cgid$' })
        if ($finished -and $TargetMod) {
            $dest = Join-Path $Instance "mods\$TargetMod\Grass"
            New-Item -ItemType Directory -Force $dest | Out-Null
            foreach ($f in $new) { Move-Item $f.FullName (Join-Path $dest ($f.BaseName + "$suffix.cgid")) -Force }
            Get-ChildItem $grassDir -File -ErrorAction SilentlyContinue | Where-Object Extension -ne '.cgid' | ForEach-Object { $_.Delete() }   # .fail markers
            Write-Step "[$s] moved $($new.Count) files into $dest as *$suffix.cgid"
        }
        $results += [pscustomobject]@{ Season = $s; Finished = $finished; Files = $new.Count; Minutes = [Math]::Round(((Get-Date) - $seasonStart).TotalMinutes, 1) }
        Write-Step "[$s] $(if ($finished) { 'finished' } else { 'NOT finished' }): $($new.Count) files"
        if (-not $finished) { break }
    }
}
finally {
    Set-SeasonsMod $seasonsWasOn
    if ($targetWasOn) { Set-ModEnabled $TargetMod $true }
    if ($null -ne $oldSeasonType) { Set-SeasonType $oldSeasonType }
    if (-not (Get-Process -Name ModOrganizer -ErrorAction SilentlyContinue)) {
        $pl = [IO.File]::ReadAllLines($pluginsTxt)
        $reactivated = @()
        $pl = $pl | ForEach-Object { if (-not $_.StartsWith('*') -and -not $_.StartsWith('#') -and $activeBefore -contains $_) { $reactivated += $_; "*$_" } else { $_ } }
        if ($reactivated) { [IO.File]::WriteAllLines($pluginsTxt, $pl); Write-Step "re-activated plugins MO2 had dropped: $($reactivated -join ', ')" }
        $t = [IO.File]::ReadAllText($mo2Ini)
        [IO.File]::WriteAllText($mo2Ini, [regex]::Replace($t, 'selected_profile=@ByteArray\([^)]*\)', "selected_profile=@ByteArray($oldSelected)"))
    }
    Write-Step "restored: Seasons of Skyrim $(if ($seasonsWasOn) { 'enabled' } else { 'disabled' }), Season Type $oldSeasonType, MO2 selected profile $oldSelected"
}
if ($TargetMod -and (Test-Path $grassDir) -and -not (Get-ChildItem $grassDir -Force)) { (Get-Item $grassDir).Delete() }
$results | Format-Table -AutoSize | Out-String | Write-Host
Write-Step ("total {0:N0} min" -f ((Get-Date) - $start).TotalMinutes)
