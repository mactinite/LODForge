<#
.SYNOPSIS
  Measures how much of a seasonal xLODGen output is identical to the default season.

.DESCRIPTION
  xLODGen writes season variants as <name>.<SPR|SUM|AUT|WIN>.<ext> next to the default <name>.<ext>.
  For each season this hashes every variant and its default counterpart and reports, per extension,
  how many files are byte-identical. Identical files are work a delta build can skip outright.
  Writes seasons-delta.json into the run directory.

.EXAMPLE
  pwsh -File Compare-Seasons.ps1 -RunDir D:\Modlists\LODForge-golden\seasons-20261017-032143
#>
param([Parameter(Mandatory)] [string] $RunDir)

$ErrorActionPreference = 'Stop'
$out = Join-Path $RunDir 'output'
$files = Get-ChildItem $out -Recurse -File
$seasonRx = '^(?<base>.+)\.(?<season>SPR|SUM|AUT|WIN)\.(?<ext>[^.]+)$'

$byPath = @{}
foreach ($f in $files) { $byPath[$f.FullName.ToLowerInvariant()] = $f }

$hashCache = @{}
function Get-Hash([string] $path) {
    if (-not $hashCache.ContainsKey($path)) { $hashCache[$path] = (Get-FileHash $path -Algorithm MD5).Hash }
    $hashCache[$path]
}

$stats = @{}
foreach ($f in $files) {
    if ($f.Name -notmatch $seasonRx) { continue }
    $season = $Matches.season.ToUpper(); $ext = $Matches.ext.ToLower()
    $default = Join-Path $f.DirectoryName "$($Matches.base).$($Matches.ext)"
    $key = "$season|$ext"
    if (-not $stats[$key]) { $stats[$key] = [ordered]@{ season = $season; ext = $ext; files = 0; identical = 0; no_default = 0; mb = 0.0; identical_mb = 0.0 } }
    $s = $stats[$key]
    $s.files++; $s.mb += $f.Length / 1MB
    if (-not $byPath.ContainsKey($default.ToLowerInvariant())) { $s.no_default++; continue }
    if ((Get-Item $default).Length -eq $f.Length -and (Get-Hash $default) -eq (Get-Hash $f.FullName)) {
        $s.identical++; $s.identical_mb += $f.Length / 1MB
    }
}

$rows = $stats.Values | Sort-Object season, ext | ForEach-Object {
    [pscustomobject]@{
        season = $_.season; ext = $_.ext; files = $_.files; identical = $_.identical
        identical_pct = if ($_.files) { [math]::Round(100 * $_.identical / $_.files, 1) } else { 0 }
        no_default = $_.no_default; mb = [math]::Round($_.mb, 1); identical_mb = [math]::Round($_.identical_mb, 1)
    }
}
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
$rows | ConvertTo-Json | Set-Content (Join-Path $RunDir 'seasons-delta.json')
