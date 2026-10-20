<#
.SYNOPSIS
  End-to-end occlusion parity test against a golden DynDOLOD run: LODForge's object height map,
  then Occlusion.esp, compared with DynDOLOD's TVDT under the tolerance policy of
  docs/re/occlusion-spec.md (section 9).

.DESCRIPTION
  1. Copies the golden default-season export with its paths pointed at the golden's saved export\
     folder (the live DynDOLOD\Edit Scripts\Export is overwritten by later runs).
  2. `lodforge object-lod --flavor dyndolod --level 4` writes <World>.objects.heightmap.dat
     (LODGen CreateHeightMap).
  3. `lodforge occlusion` with that height map and the run's DynDOLOD.esm/.esp, compared with the
     golden Occlusion.esp: the cells selected per worldspace, and the visible LOD4 blocks per cell
     (TVDT leaves) as "hidden by us but visible in golden" (LOD holes) and "visible to us only"
     (extra draws), both as a percentage of the golden's visible blocks.
  4. The same run with the golden's own height map (isolates the visibility model from the
     height-map port); its numbers are recorded, not gated.
  Writes summary.json in -Out and exits 1 when the cell selection differs from the golden's,
  when the hidden percentage exceeds -MaxHiddenPercent or the extra percentage exceeds
  -MaxExtraPercent. Byte identity with the golden plugin is no longer a goal (the visibility
  model is fitted to output, not derived from the reference); -RequireIdentical keeps the old
  check for a run that wants it. Needs no MO2 or game; reads the instance's load order.

.EXAMPLE
  pwsh -File scripts\golden\dyndolod\Test-OcclusionParity.ps1
  pwsh -File scripts\golden\dyndolod\Test-OcclusionParity.ps1 -MaxHiddenPercent 6
#>
[CmdletBinding()]
param(
    [string] $Golden = 'D:\Modlists\LODForge-golden\dyndolod-20261017-medium-tamriel',
    [string] $Instance = 'D:\Modlists\skyrim-cs',
    [string] $Mo2Profile = 'LODForge-DynDOLOD',
    [string] $World = 'Tamriel',
    [string] $Lodforge = (Join-Path $PSScriptRoot '..\..\..\target\release\lodforge.exe'),
    [string] $Out = (Join-Path $PSScriptRoot '..\..\..\bench\occlusion-test'),
    # Tolerance (spec section 9): blocks the golden shows that we hide, as a percentage of the
    # golden's visible LOD4 blocks over all worldspaces (recorded 2026-10-20: see the spec).
    [double] $MaxHiddenPercent = 8.0,
    # Blocks we show that the golden hides, same denominator.
    [double] $MaxExtraPercent = 12.0,
    [switch] $RequireIdentical
)

$ErrorActionPreference = 'Stop'
$Lodforge = (Resolve-Path $Lodforge).Path
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$exportDir = Join-Path $Golden 'export'
$t0 = Get-Date

# 1. Export copy with paths into the golden export folder.
$src = Join-Path $exportDir "LODGen_SSE_Export_$World.txt"
$liveExport = (Select-String -Path $src -Pattern '^TerrainData=(.*\\)[^\\]+$' | Select-Object -First 1).Matches[0].Groups[1].Value
$export = Join-Path $Out "LODGen_SSE_Export_$World.txt"
$text = [IO.File]::ReadAllText($src).Replace($liveExport, "$exportDir\")
[IO.File]::WriteAllText($export, $text)

# 2. Object LOD height map.
$lod = Join-Path $Out 'objlod'
& $Lodforge object-lod --instance $Instance --profile $Mo2Profile --flavor dyndolod --export $export --level 4 --out $lod 2>&1 | Out-File (Join-Path $Out 'object-lod.log')
if ($LASTEXITCODE -ne 0) { throw "object-lod failed ($LASTEXITCODE), see object-lod.log" }
$heightmap = Get-ChildItem $lod -Recurse -Filter "$World.objects.heightmap.dat" | Select-Object -First 1
if (-not $heightmap) { throw 'object-lod wrote no heightmap' }

# Parses the `all worldspaces ...` summary lines of an occlusion log.
function Read-Summary([string] $log) {
    $all = Select-String -Path $log -Pattern 'all worldspaces vs golden: (\d+)/(\d+) cells identical' | Select-Object -Last 1
    if (-not $all) { throw "no golden summary in $log" }
    $blocks = Select-String -Path $log -Pattern 'all worldspaces visible LOD4 blocks: golden (\d+), ours (\d+), hidden by us but visible in golden (\d+) \(([\d.]+)%\), visible to us only (\d+) \(([\d.]+)%\), block Jaccard ([\d.]+), cells only ours (\d+), cells only golden (\d+)' | Select-Object -Last 1
    if (-not $blocks) { throw "no block summary in $log" }
    $g = $blocks.Matches[0].Groups
    [ordered]@{
        tvdt_identical = [int]$all.Matches[0].Groups[1].Value; tvdt_total = [int]$all.Matches[0].Groups[2].Value
        golden_blocks = [int]$g[1].Value; our_blocks = [int]$g[2].Value
        hidden_blocks = [int]$g[3].Value; hidden_percent = [double]$g[4].Value
        extra_blocks = [int]$g[5].Value; extra_percent = [double]$g[6].Value
        block_jaccard = [double]$g[7].Value
        cells_only_ours = [int]$g[8].Value; cells_only_golden = [int]$g[9].Value
    }
}

# 3. Occlusion with LODForge's height map.
$esp = Join-Path $Out 'Occlusion.esp'
$goldenEsp = Join-Path $Golden 'output\Occlusion.esp'
$occLog = Join-Path $Out 'occlusion.log'
& $Lodforge occlusion --instance $Instance --profile $Mo2Profile --world $World --heightmap $heightmap.FullName `
    --extra-plugin (Join-Path $Golden 'output\DynDOLOD.esm') --extra-plugin (Join-Path $Golden 'output\DynDOLOD.esp') `
    --golden $goldenEsp --out $esp 2>&1 | Out-File $occLog
if ($LASTEXITCODE -ne 0) { throw "occlusion failed ($LASTEXITCODE), see occlusion.log" }
$ours = Read-Summary $occLog

# Bytes, for the record. Returns (first differing offset or -1, differing byte count incl. length difference).
function Compare-Bytes([string] $x, [string] $y) {
    $a = [IO.File]::ReadAllBytes($x); $b = [IO.File]::ReadAllBytes($y)
    $n = [Math]::Min($a.Length, $b.Length); $first = -1; $count = [Math]::Abs($a.Length - $b.Length)
    for ($i = 0; $i -lt $n; $i++) { if ($a[$i] -ne $b[$i]) { $count++; if ($first -lt 0) { $first = $i } } }
    if ($first -lt 0 -and $a.Length -ne $b.Length) { $first = $n }
    , @($first, $count, $a.Length, $b.Length)
}
$cmp = Compare-Bytes $esp $goldenEsp
$identical = $cmp[0] -lt 0

# 4. The same run with DynDOLOD's own height map (the visibility model alone).
$goldenHm = Join-Path $Golden "output\meshes\Terrain\$World\Objects\$World.objects.heightmap.dat"
$withGoldenLog = Join-Path $Out 'occlusion.golden-heightmap.log'
& $Lodforge occlusion --instance $Instance --profile $Mo2Profile --world $World --heightmap $goldenHm `
    --extra-plugin (Join-Path $Golden 'output\DynDOLOD.esm') --extra-plugin (Join-Path $Golden 'output\DynDOLOD.esp') `
    --golden $goldenEsp --out (Join-Path $Out 'Occlusion.golden-heightmap.esp') 2>&1 | Out-File $withGoldenLog
if ($LASTEXITCODE -ne 0) { throw "occlusion (golden height map) failed ($LASTEXITCODE)" }
$withGolden = Read-Summary $withGoldenLog

$summary = [ordered]@{
    golden = $Golden; world = $World
    tvdt_identical = $ours.tvdt_identical; tvdt_total = $ours.tvdt_total
    golden_blocks = $ours.golden_blocks; our_blocks = $ours.our_blocks
    hidden_blocks = $ours.hidden_blocks; hidden_percent = $ours.hidden_percent
    extra_blocks = $ours.extra_blocks; extra_percent = $ours.extra_percent
    block_jaccard = $ours.block_jaccard
    cells_only_ours = $ours.cells_only_ours; cells_only_golden = $ours.cells_only_golden
    with_golden_heightmap = $withGolden
    esp_bytes = $cmp[2]; golden_bytes = $cmp[3]; esp_identical = $identical; differing_bytes = $cmp[1]
    max_hidden_percent = $MaxHiddenPercent; max_extra_percent = $MaxExtraPercent
    seconds = [Math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
}
$summary | ConvertTo-Json | Set-Content (Join-Path $Out 'summary.json')
"TVDT {0}/{1} identical; visible LOD4 blocks: golden {2}, ours {3}, hidden by us {4} ({5}%), extra {6} ({7}%), block Jaccard {8}; cells only ours {9}, only golden {10}; with DynDOLOD's height map: hidden {11}%, extra {12}%; Occlusion.esp {13} vs {14} bytes; {15} s" -f `
    $ours.tvdt_identical, $ours.tvdt_total, $ours.golden_blocks, $ours.our_blocks, $ours.hidden_blocks, $ours.hidden_percent, `
    $ours.extra_blocks, $ours.extra_percent, $ours.block_jaccard, $ours.cells_only_ours, $ours.cells_only_golden, `
    $withGolden.hidden_percent, $withGolden.extra_percent, $cmp[2], $cmp[3], $summary.seconds

$fail = ($ours.cells_only_ours -ne 0) -or ($ours.cells_only_golden -ne 0) -or `
    ($ours.hidden_percent -gt $MaxHiddenPercent) -or ($ours.extra_percent -gt $MaxExtraPercent) -or `
    ($RequireIdentical -and -not $identical)
if ($fail) { Write-Host 'FAIL'; exit 1 } else { Write-Host 'PASS' }
