<#
.SYNOPSIS
  Switches the LODForge-E2E profile between reference output and LODForge's output,
  and forces a Seasons of Skyrim season, for side-by-side in-game comparisons.

.DESCRIPTION
  DynDOLOD: "LODForge E2E - DynDOLOD Output" + "LODForge E2E - TexGen Output" on, their plugins
            (DynDOLOD.esm, DynDOLOD.esp, Occlusion.esp) active, "LODForge E2E - LODForge Output" off.
  LODForge: "LODForge E2E - LODForge Output" on, the DynDOLOD and TexGen outputs and their plugins off.
  Season: Default leaves Season Type at 5 (calendar); WIN/SPR/SUM/AUT force 1/2/3/4 in
  overwrite\SKSE\Plugins\po3_SeasonsOfSkyrim.ini. MO2 must be closed. -Restore puts back the
  *.before-compare backups.
#>
[CmdletBinding()]
param(
    [ValidateSet('DynDOLOD', 'LODForge')] [string] $Config,
    [ValidateSet('Default', 'WIN', 'SPR', 'SUM', 'AUT')] [string] $Season = 'Default',
    [string] $Instance = 'D:\Modlists\skyrim-cs',
    [string] $Mo2Profile = 'LODForge-E2E',
    [switch] $Restore
)
$ErrorActionPreference = 'Stop'
if (Get-Process -Name ModOrganizer, SkyrimSE -ErrorAction SilentlyContinue) { throw 'MO2 or the game is running' }
$p = Join-Path $Instance "profiles\$Mo2Profile"
$sos = Join-Path $Instance 'overwrite\SKSE\Plugins\po3_SeasonsOfSkyrim.ini'
if ($Restore) {
    foreach ($f in "$p\modlist.txt", "$p\plugins.txt", $sos) { Copy-Item "$f.before-compare" $f -Force }
    'restored modlist.txt, plugins.txt and po3_SeasonsOfSkyrim.ini'; return
}
$dyn = $Config -eq 'DynDOLOD'
$mods = @{ 'LODForge E2E - LODForge Output' = -not $dyn; 'LODForge E2E - DynDOLOD Output' = $dyn; 'LODForge E2E - TexGen Output' = $dyn }
$ml = [IO.File]::ReadAllLines("$p\modlist.txt") | ForEach-Object {
    $name = $_.Substring([Math]::Min(1, $_.Length)); if ($mods.ContainsKey($name)) { $(if ($mods[$name]) { "+$name" } else { "-$name" }) } else { $_ } }
[IO.File]::WriteAllLines("$p\modlist.txt", $ml)
$plugins = 'DynDOLOD.esm', 'DynDOLOD.esp', 'Occlusion.esp'
$pl = @([IO.File]::ReadAllLines("$p\plugins.txt") | Where-Object { $plugins -notcontains $_.TrimStart('*') })
if ($dyn) { $pl += $plugins | ForEach-Object { "*$_" } }
[IO.File]::WriteAllLines("$p\plugins.txt", $pl)
$type = @{ Default = 5; WIN = 1; SPR = 2; SUM = 3; AUT = 4 }[$Season]
$t = [IO.File]::ReadAllText($sos)
[IO.File]::WriteAllText($sos, [regex]::Replace($t, '(?m)^Season Type\s*=\s*\d+', "Season Type = $type"))
"config $Config, season $Season (Season Type $type)"
Select-String "$p\modlist.txt" -Pattern 'LODForge E2E' | ForEach-Object { "  $($_.Line)" }
