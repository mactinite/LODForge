# Prints the last lines of a running xEdit-based tool's log memo (TMemo in the main window) and the
# text of any task dialog it has open.
param([string]$ProcName = 'DynDOLODx64', [int]$Lines = 40)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
Add-Type -Namespace LodForge -Name L -MemberDefinition '
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, System.Text.StringBuilder l);'
$p = Get-Process $ProcName | Select-Object -First 1
$main = $UIA::RootElement.FindFirst($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))
$memo = $main.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, 'TMemo')))
if ($memo) {
    $h = [IntPtr]$memo.Current.NativeWindowHandle
    $len = [int][LodForge.L]::SendMessage($h, 0x0E, [IntPtr]::Zero, [IntPtr]::Zero)
    $sb = New-Object Text.StringBuilder ($len + 2); $null = [LodForge.L]::SendMessage($h, 0x0D, [IntPtr]($len + 1), $sb)
    $sb.ToString() -split "`r?`n" | Where-Object { $_ -ne '' } | Select-Object -Last $Lines
}
foreach ($d in $main.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, '#32770')))) {
    "--- dialog '$($d.Current.Name)'"
    foreach ($e in $d.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition)) { if ($e.Current.Name) { "  [$($e.Current.ClassName)] $($e.Current.Name)" } }
}
