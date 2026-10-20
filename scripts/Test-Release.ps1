<#
.SYNOPSIS
Fail closed unless this exact tagged source has maintainer licensing clearance.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Tag,
    [AllowEmptyString()]
    [string] $ApprovedCommit = ''
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if ($Tag -cnotmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw 'Release tag must be vX.Y.Z with no leading zeroes.'
}
$commit = git rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve HEAD.' }
if ($ApprovedCommit -cnotmatch '^[0-9a-f]{40}$' -or $ApprovedCommit -cne $commit) {
    throw 'Release blocked: RELEASE_APPROVED_COMMIT must equal the full reviewed commit SHA. See docs/licensing.md.'
}
$tagCommit = git rev-parse --verify "refs/tags/$Tag^{commit}"
if ($LASTEXITCODE -ne 0 -or $tagCommit -cne $commit) {
    throw 'Release tag does not identify the checked-out commit.'
}
git diff --quiet HEAD --
if ($LASTEXITCODE -ne 0) { throw 'Release source has uncommitted changes.' }

$metadata = cargo metadata --locked --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'Cannot read Cargo package versions.' }
$packages = ($metadata | ConvertFrom-Json).packages
$version = $Tag.Substring(1)
foreach ($name in @('lodforge', 'lodforge-ui')) {
    $package = @($packages | Where-Object name -EQ $name)
    if ($package.Count -ne 1 -or $package[0].version -cne $version) {
        throw "$name version does not match $Tag."
    }
}
$config = Get-Content -LiteralPath 'apps\ui\tauri.conf.json' -Raw | ConvertFrom-Json
if ($config.version -cne $version) { throw "Tauri version does not match $Tag." }

foreach ($path in @('LICENSE', 'THIRD-PARTY-NOTICES.txt')) {
    git cat-file -e "HEAD:$path"
    if ($LASTEXITCODE -ne 0) { throw "Release blocked: committed $path is required after licensing review." }
    if ([string]::IsNullOrWhiteSpace((Get-Content -LiteralPath $path -Raw))) {
        throw "Release blocked: $path is empty."
    }
}
$licenses = @(git ls-tree -r --name-only HEAD -- licenses)
if ($LASTEXITCODE -ne 0 -or $licenses.Count -eq 0) {
    throw 'Release blocked: committed upstream license texts in licenses are required.'
}
Write-Host "Release clearance verified for $Tag ($commit). This gate records approval; it is not a legal audit."
