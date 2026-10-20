# Presses a button of a task dialog (#32770 with CCPushButton) that a tool has open, by caption,
# through UI Automation's InvokePattern (what a real click does). Do not use TDM_CLICK_BUTTON with
# GetDlgCtrlID: task-dialog buttons report id 0 there, and id 0 dismisses the dialog without a choice
# (a reference run lost its generated plugins that way).
param([string]$ProcName = 'DynDOLODx64', [string]$Name = 'Exit DynDOLOD', [int]$WaitSeconds = 30)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
$p = Get-Process $ProcName | Select-Object -First 1
$btn = $null
$end = (Get-Date).AddSeconds(10)
while (-not $btn -and (Get-Date) -lt $end) {
    foreach ($w in $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))) {
        $btn = $w.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'CCPushButton'))) | Where-Object { $_.Current.Name -eq $Name } | Select-Object -First 1
        if ($btn) { break }
    }
    if (-not $btn) { Start-Sleep 1 }
}
if (-not $btn) { throw "no task dialog button '$Name'" }
# Task-dialog buttons are windowless (no hwnd, no control id) and have no InvokePattern; the MSAA
# default action (LegacyIAccessible) presses them.
$pat = $null
if ($btn.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) { $pat.Invoke() }
else {
    $legacy = [Windows.Automation.AutomationPattern]::LookupById(10018)   # LegacyIAccessiblePattern
    if (-not $legacy) { throw 'LegacyIAccessiblePattern not available' }
    $lp = $btn.GetCurrentPattern($legacy)
    $lp.GetType().InvokeMember('DoDefaultAction', [Reflection.BindingFlags]::InvokeMethod, $null, $lp, $null) | Out-Null
}
Start-Sleep 2
$still = $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id))) | ForEach-Object { $_.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'CCPushButton'))) } | Where-Object { $_.Current.Name -eq $Name }
if ($still) { throw "dialog with '$Name' is still open" }
$end = (Get-Date).AddSeconds($WaitSeconds)
while ((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -and (Get-Date) -lt $end) { Start-Sleep 1 }
"pressed '$Name'; process $(if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) { 'still running' } else { 'exited' })"
