<#
.SYNOPSIS
Run repository Rust quality checks without rewriting source files.
.EXAMPLE
.\scripts\Check.ps1 -Check Test,Lint -Package lodforge-objlod,lodforge-dyndolod
.EXAMPLE
.\scripts\Check.ps1 -Check Docs
#>
[CmdletBinding()]
param(
    [ValidateSet('Format', 'Lint', 'Test', 'Docs')]
    [string[]] $Check = @('Test'),
    [string[]] $Package = @()
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    Get-Command cargo -ErrorAction Stop | Out-Null
    foreach ($name in $Check) {
        $scope = @()
        if ($Package.Count -gt 0) {
            foreach ($item in $Package) {
                $scope += @('--package', $item)
            }
        } elseif ($name -eq 'Format') {
            $scope = @('--all')
        } else {
            $scope = @('--workspace', '--exclude', 'lodforge-ui')
        }

        $cargoArgs = switch ($name) {
            'Format' { @('fmt') + $scope + @('--', '--check') }
            'Lint' { @('clippy', '--locked') + $scope + @('--all-targets') }
            'Test' { @('test', '--locked') + $scope }
            'Docs' { @('doc', '--locked', '--no-deps') + $scope }
        }
        Write-Host "== cargo $($cargoArgs -join ' ')"
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw "$name check failed (cargo exit $LASTEXITCODE)."
        }
    }
} finally {
    Pop-Location
}
