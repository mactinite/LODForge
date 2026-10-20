param([string[]]$Plugins)
$inst = 'D:\Modlists\skyrim-cs'
$inv = Join-Path $PSScriptRoot 'Click-Button.ps1'
foreach ($Plugin in $Plugins) {
    $a = "-sse -qac -autoexit -autoload \`"$Plugin\`" -d:\`"$inst\Stock Game\Data\`""
    Start-Process "$inst\ModOrganizer.exe" -ArgumentList "-p `"LODForge-DynDOLOD`" run -e -a `"$a`" `"SSEEditQuickAutoClean`""
    $t0 = Get-Date; $closed = $false
    Start-Sleep 5
    while ((Get-Process SSEEditQuickAutoClean, ModOrganizer -ErrorAction SilentlyContinue)) {
        if (((Get-Date) - $t0).TotalMinutes -gt 30) { "timeout"; break }
        if (-not $closed -and (Get-Process SSEEditQuickAutoClean -ErrorAction SilentlyContinue)) {
            $r = pwsh -File $inv -ProcName SSEEditQuickAutoClean -Name 'Close' 2>$null
            if ($r -match 'sent|invoked') { $closed = $true }
        }
        Start-Sleep 3
    }
    $log = Get-Content "$inst\tools\SSEEdit 4.1.5f\SSEEdit_log.txt" | Select-String 'Undeleted Records|Removed Records|Done saving|Error' | ForEach-Object { $_.Line }
    "$Plugin done in $([int]((Get-Date)-$t0).TotalSeconds)s"; $log
}
