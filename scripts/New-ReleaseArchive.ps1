<#
.SYNOPSIS
Package a private portable build and smoke-test the extracted CLI. Does not authorize distribution.
#>
[CmdletBinding()]
param(
    [string] $BinaryDirectory = 'target\release',
    [string] $Destination = 'target\dist'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
$stage = $null
$unpack = $null
try {
    $metadata = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Cargo package versions.' }
    $packages = ($metadata | ConvertFrom-Json).packages
    $version = ($packages | Where-Object name -EQ 'lodforge').version
    $uiVersion = ($packages | Where-Object name -EQ 'lodforge-ui').version
    $config = Get-Content -LiteralPath 'apps\ui\tauri.conf.json' -Raw | ConvertFrom-Json
    if ($version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
        $uiVersion -cne $version -or $config.version -cne $version) {
        throw 'CLI, UI and Tauri versions must agree and use X.Y.Z.'
    }
    $destinationPath = if ([IO.Path]::IsPathRooted($Destination)) {
        [IO.Path]::GetFullPath($Destination)
    } else {
        [IO.Path]::GetFullPath((Join-Path $root $Destination))
    }
    New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
    $archive = Join-Path $destinationPath "LODForge-v$version-windows-x64.zip"
    $checksums = Join-Path $destinationPath 'SHA256SUMS.txt'
    if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath $checksums)) {
        throw 'Destination already contains release output. Use a new destination rather than overwriting it.'
    }
    $stage = Join-Path ([IO.Path]::GetTempPath()) "lodforge-package-$([guid]::NewGuid())"
    $unpack = Join-Path ([IO.Path]::GetTempPath()) "lodforge-unpack-$([guid]::NewGuid())"
    New-Item -ItemType Directory -Path $stage | Out-Null
    foreach ($name in @('lodforge.exe', 'lodforge-ui.exe')) {
        $binary = Join-Path $BinaryDirectory $name
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing $binary." }
        Copy-Item -LiteralPath $binary -Destination $stage
    }
    foreach ($path in @('README.md', 'PLAN.md', 'docs')) {
        Copy-Item -LiteralPath $path -Destination $stage -Recurse
    }
    foreach ($path in @('LICENSE', 'THIRD-PARTY-NOTICES.txt', 'licenses')) {
        if (Test-Path -LiteralPath $path) {
            Copy-Item -LiteralPath $path -Destination $stage -Recurse
        } else {
            Write-Warning "$path is absent: this is an internal test package, NOT cleared for distribution."
        }
    }
    $commit = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record source revision.' }
    $changes = git status --porcelain
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record working tree state.' }
    $rust = rustc --version
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record Rust version.' }
    [ordered]@{
        version = $version
        commit = $commit
        dirty = [bool]$changes
        target = 'x86_64-pc-windows-msvc'
        rust = $rust
        source = "https://github.com/mactinite/LODForge/tree/$commit"
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'build-info.json') -Encoding utf8
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $unpack
    $cli = Join-Path $unpack 'lodforge.exe'
    $actualVersion = & $cli --version
    if ($LASTEXITCODE -ne 0 -or $actualVersion -cne "lodforge $version") {
        throw 'Packaged CLI version smoke test failed.'
    }
    & $cli --help | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Packaged CLI help smoke test failed.' }
    & $cli build --help | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Packaged build help smoke test failed.' }
    if (-not (Test-Path -LiteralPath (Join-Path $unpack 'lodforge-ui.exe') -PathType Leaf)) {
        throw 'Packaged UI executable is missing.'
    }
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $(Split-Path $archive -Leaf)" | Set-Content -LiteralPath $checksums -Encoding ascii
    Write-Host "Verified portable archive: $archive"
} finally {
    foreach ($temporary in @($stage, $unpack)) {
        if ($temporary -and (Test-Path -LiteralPath $temporary)) {
            Remove-Item -LiteralPath $temporary -Recurse -Force
        }
    }
    Pop-Location
}
