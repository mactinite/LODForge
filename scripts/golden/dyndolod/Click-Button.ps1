param([string]$ProcName, [string]$Name)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
Add-Type -Namespace LodForge -Name W3 -MemberDefinition '
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h, int m, IntPtr w, IntPtr l, int f, int t, out IntPtr r);'
$p = Get-Process $ProcName | Select-Object -First 1
$wins = $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, [int]$p.Id)))
foreach ($w in $wins) {
    foreach ($e in $w.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
        if ($e.Current.Name -ne $Name) { continue }
        "found $($e.Current.ClassName) patterns: $(($e.GetSupportedPatterns() | % ProgrammaticName) -join ',')"
        $ip = $null
        if ($e.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern, [ref]$ip)) { $ip.Invoke(); "invoked"; return }
        $h = [IntPtr]$e.Current.NativeWindowHandle; $r = [IntPtr]::Zero
        $null = [LodForge.W3]::SendMessageTimeout($h, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero, 2, 5000, [ref]$r); "sent BM_CLICK"; return
    }
}
"not found: $Name"
