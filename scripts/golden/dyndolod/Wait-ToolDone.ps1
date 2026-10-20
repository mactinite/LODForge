# Waits until a running tool's log memo shows a completion marker or the tool opens a task dialog
# (end-of-run choices or a fatal error), polling Get-ToolLog.ps1. Prints the final log lines.
param([string]$ProcName = 'DynDOLODx64', [string]$DoneRegex = 'completed successfully|Errors have occured', [int]$TimeoutMinutes = 240)
$t0 = Get-Date
while ($true) {
    if (-not (Get-Process $ProcName -ErrorAction SilentlyContinue)) { 'tool exited'; break }
    $o = & (Join-Path $PSScriptRoot 'Get-ToolLog.ps1') -ProcName $ProcName -Lines 12 2>&1
    if (($o -join "`n") -match $DoneRegex -or ($o -match '^--- dialog')) { break }
    if (((Get-Date) - $t0).TotalMinutes -gt $TimeoutMinutes) { 'timeout'; break }
    Start-Sleep 15
}
"waited {0:N1} min" -f ((Get-Date) - $t0).TotalMinutes
$o
