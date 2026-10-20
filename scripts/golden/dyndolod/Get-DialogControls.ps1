# Waits for a dialog (window class) of a running tool and prints its controls: class, name, hwnd,
# position, enabled and, for check boxes / radio buttons, the checked state (BM_GETCHECK).
param([string]$ProcName = 'TexGenx64', [string]$DialogClass = 'TfrmTexGen', [int]$WaitSeconds = 600)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
Add-Type -Namespace LodForge -Name D -MemberDefinition '[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, IntPtr l);'
$end = (Get-Date).AddSeconds($WaitSeconds)
$dlg = $null
while (-not $dlg -and (Get-Date) -lt $end) {
    $p = Get-Process $ProcName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($p) {
        $wins = $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))
        foreach ($w in $wins) {
            if ($w.Current.ClassName -eq $DialogClass) { $dlg = $w; break }
            $d = $w.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $DialogClass)))
            if ($d) { $dlg = $d; break }
        }
    }
    if (-not $dlg) { Start-Sleep 3 }
}
if (-not $dlg) { throw "no $DialogClass within $WaitSeconds s" }
"dialog '$($dlg.Current.Name)' hwnd=$($dlg.Current.NativeWindowHandle)"
foreach ($e in $dlg.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
    $c = $e.Current; $r = $c.BoundingRectangle
    $state = if ($c.ClassName -in 'TCheckBox', 'TRadioButton') { ' checked=' + [int][LodForge.D]::SendMessage([IntPtr]$c.NativeWindowHandle, 0xF0, [IntPtr]::Zero, [IntPtr]::Zero) } else { '' }
    '{0} | {1} | hwnd={2} | {3},{4} | en={5}{6}' -f $c.ClassName, $c.Name, $c.NativeWindowHandle, [int]$r.X, [int]$r.Y, $c.IsEnabled, $state
}
