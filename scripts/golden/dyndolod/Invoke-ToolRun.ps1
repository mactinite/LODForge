# Attach to a running reference tool whose options dialog is up: record options, click a button,
# sample caption + last memo line until the memo shows a completion marker or the process exits.
param([string]$ProcName = 'TexGenx64', [string]$DialogClass = 'TfrmTexGen', [string]$Button = 'Start',
      [string]$RunDir = 'D:\Modlists\LODForge-golden\texgen-probe', [string]$DoneRegex = 'completed successfully|Completed|finished', [int]$TimeoutMinutes = 300)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
Add-Type -Namespace LodForge -Name W -MemberDefinition '
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, System.Text.StringBuilder l);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);'
New-Item -ItemType Directory -Force $RunDir | Out-Null
$p = Get-Process $ProcName | Select-Object -First 1
$main = $UIA::RootElement.FindFirst($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))
$dlg = $main.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $DialogClass)))
if (-not $dlg) { throw "dialog $DialogClass not found" }
$opts = [ordered]@{}
foreach ($e in $dlg.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
    $c = $e.Current; $h = [IntPtr]$c.NativeWindowHandle
    if ($c.ClassName -eq 'TCheckBox' -or $c.ClassName -eq 'TRadioButton') {
        $opts["$($c.ClassName):$($c.Name)@$([int]$c.BoundingRectangle.Y)"] = [int][LodForge.W]::SendMessage($h, 0xF0, [IntPtr]::Zero, [IntPtr]::Zero) }
    elseif ($c.ClassName -in 'TComboBox', 'TEdit') {
        $sb = New-Object Text.StringBuilder 1024; $null = [LodForge.W]::SendMessage($h, 0x0D, [IntPtr]1024, $sb)
        $opts["$($c.ClassName)@$([int]$c.BoundingRectangle.X),$([int]$c.BoundingRectangle.Y)"] = $sb.ToString() }
}
$opts | ConvertTo-Json | Set-Content (Join-Path $RunDir 'options.json')
$btn = $dlg.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'TButton'))) | Where-Object { $_.Current.Name -eq $Button } | Select-Object -First 1
$memo = $main.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'TMemo')))
$memoH = [IntPtr]$memo.Current.NativeWindowHandle
function MemoTail([int]$n) {
    $len = [int][LodForge.W]::SendMessage($memoH, 0x0E, [IntPtr]::Zero, [IntPtr]::Zero)
    $sb = New-Object Text.StringBuilder ($len + 2); $null = [LodForge.W]::SendMessage($memoH, 0x0D, [IntPtr]($len + 1), $sb)
    $lines = $sb.ToString() -split "`r?`n" | Where-Object { $_ -ne '' }
    $lines | Select-Object -Last $n
}
$timeline = Join-Path $RunDir 'timeline.csv'
'utc,elapsed_s,cpu_s,ws_mb,children,last_line' | Set-Content $timeline
$null = [LodForge.W]::PostMessage([IntPtr]$btn.Current.NativeWindowHandle, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero)
$t0 = Get-Date; "start_utc=$($t0.ToUniversalTime().ToString('o'))" | Set-Content (Join-Path $RunDir 'run.txt')
$last = ''; $peak = 0
while ($true) {
    Start-Sleep -Seconds 5
    if ($p.HasExited) { 'exited' | Add-Content (Join-Path $RunDir 'run.txt'); break }
    $p.Refresh()
    $tail = @(MemoTail 3)
    $ll = $tail[-1]
    $ch = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)").Count
    $ws = [math]::Round($p.WorkingSet64 / 1MB); if ($ws -gt $peak) { $peak = $ws }
    $el = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
    '{0},{1},{2},{3},{4},"{5}"' -f (Get-Date).ToUniversalTime().ToString('o'), $el, [math]::Round($p.TotalProcessorTime.TotalSeconds, 1), $ws, $ch, ($ll -replace '"', "'") | Add-Content $timeline
    if (($tail -join "`n") -match $DoneRegex) { "done_line=$ll" | Add-Content (Join-Path $RunDir 'run.txt'); break }
    if (((Get-Date) - $t0).TotalMinutes -gt $TimeoutMinutes) { 'timeout' | Add-Content (Join-Path $RunDir 'run.txt'); break }
}
$t1 = Get-Date
"end_utc=$($t1.ToUniversalTime().ToString('o'))`nseconds=$([math]::Round(($t1-$t0).TotalSeconds,1))`ncpu_s=$([math]::Round($p.TotalProcessorTime.TotalSeconds,1))`npeak_ws_mb=$peak" | Add-Content (Join-Path $RunDir 'run.txt')
MemoTail 400 | Set-Content (Join-Path $RunDir 'memo-tail.txt')
