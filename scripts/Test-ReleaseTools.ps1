<#
.SYNOPSIS
Exercise release guard success and failure paths with synthetic metadata (no game assets).
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$guard = Join-Path $PSScriptRoot 'Test-Release.ps1'
$fixture = Join-Path ([IO.Path]::GetTempPath()) "lodforge-release-tests-$([guid]::NewGuid())"
$script:commit = 'a' * 40
$script:passed = 0

$state = @{
    commit = $script:commit
    tagCommit = $script:commit
    version = '0.1.0'
    uiVersion = '0.1.0'
    gitDirty = $false
    missingNotice = $false
    missingLicenses = $false
    metadataFailure = $false
}

Set-Item Function:git -Value ({
    $global:LASTEXITCODE = 0
    switch ($args[0]) {
        'rev-parse' {
            if ($args[1] -eq 'HEAD') { return $state.commit }
            return $state.tagCommit
        }
        'diff' { if ($state.gitDirty) { $global:LASTEXITCODE = 1 }; return }
        'cat-file' { if ($state.missingNotice) { $global:LASTEXITCODE = 1 }; return }
        'ls-tree' { if (-not $state.missingLicenses) { return 'licenses/test.txt' }; return }
        default { throw "Unexpected git invocation: $args" }
    }
}.GetNewClosure())

Set-Item Function:cargo -Value ({
    $global:LASTEXITCODE = 0
    if ($state.metadataFailure) { $global:LASTEXITCODE = 1; return }
    return (@{
        packages = @(
            @{ name = 'lodforge'; version = $state.version },
            @{ name = 'lodforge-ui'; version = $state.uiVersion }
        )
    } | ConvertTo-Json -Depth 4)
}.GetNewClosure())

function Assert-Blocked([string] $Name, [scriptblock] $Action, [string] $Message) {
    $caught = $null
    try { & $Action } catch { $caught = $_.Exception.Message }
    if (-not $caught -or $caught -notlike "*$Message*") {
        throw "$Name failed: expected '$Message', got '$caught'."
    }
    $script:passed++
    Write-Host "PASS: $Name"
}

New-Item -ItemType Directory -Path (Join-Path $fixture 'apps\ui') -Force | Out-Null
Push-Location $fixture
try {
    '{"version":"0.1.0"}' | Set-Content -LiteralPath 'apps\ui\tauri.conf.json'
    'Synthetic test license, not a project license.' | Set-Content -LiteralPath LICENSE
    'Synthetic test notices.' | Set-Content -LiteralPath THIRD-PARTY-NOTICES.txt
    & $guard -Tag v0.1.0 -ApprovedCommit $script:commit
    $script:passed++
    Write-Host 'PASS: cleared matching source'

    Assert-Blocked 'missing approval' { & $guard -Tag v0.1.0 } 'RELEASE_APPROVED_COMMIT'
    Assert-Blocked 'wrong approval' { & $guard -Tag v0.1.0 -ApprovedCommit ('b' * 40) } 'RELEASE_APPROVED_COMMIT'
    foreach ($tag in @('0.1.0', 'v01.1.0', 'v0.1.0-rc.1', 'v0.1.0;echo bad')) {
        Assert-Blocked "invalid tag $tag" { & $guard -Tag $tag -ApprovedCommit $script:commit } 'Release tag must'
    }
    $state.tagCommit = 'b' * 40
    Assert-Blocked 'moved tag' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'tag does not identify'
    $state.tagCommit = $script:commit
    $state.gitDirty = $true
    Assert-Blocked 'dirty source' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'uncommitted'
    $state.gitDirty = $false
    $state.version = '0.2.0'
    Assert-Blocked 'CLI version mismatch' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'lodforge version'
    $state.version = '0.1.0'
    $state.uiVersion = '0.2.0'
    Assert-Blocked 'UI version mismatch' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'lodforge-ui version'
    $state.uiVersion = '0.1.0'
    '{"version":"0.2.0"}' | Set-Content -LiteralPath 'apps\ui\tauri.conf.json'
    Assert-Blocked 'Tauri version mismatch' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'Tauri version'
    '{"version":"0.1.0"}' | Set-Content -LiteralPath 'apps\ui\tauri.conf.json'
    $state.metadataFailure = $true
    Assert-Blocked 'metadata failure' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'Cannot read Cargo'
    $state.metadataFailure = $false
    $state.missingNotice = $true
    Assert-Blocked 'uncommitted legal documents' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'committed LICENSE'
    $state.missingNotice = $false
    '' | Set-Content -LiteralPath LICENSE
    Assert-Blocked 'empty license' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'LICENSE is empty'
    'Synthetic test license.' | Set-Content -LiteralPath LICENSE
    $state.missingLicenses = $true
    Assert-Blocked 'missing upstream license texts' { & $guard -Tag v0.1.0 -ApprovedCommit $script:commit } 'upstream license texts'
    Write-Host "$script:passed release guard checks passed."
} finally {
    Pop-Location
    Remove-Item -LiteralPath $fixture -Recurse -Force
}
