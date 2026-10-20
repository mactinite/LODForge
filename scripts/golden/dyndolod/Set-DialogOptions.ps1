# Sets check boxes (by caption) to a wanted state with a real BM_CLICK, so the VCL OnClick logic
# runs, and edit boxes (by current-text prefix or index) with WM_SETTEXT, in a tool's dialog.
#   -Check 'Tree=1','Rendered=1'   -Edit 'D:\old\=D:\new\'   (prefix of the current text = new text)
#   -Press 'Advanced >>>'           (a TButton by caption, pressed last, with PostMessage so a modal
#                                    dialog it opens does not block this script)
param([string]$ProcName = 'TexGenx64', [string]$DialogClass = 'TfrmTexGen', [string[]]$Check = @(), [string[]]$Edit = @(), [string]$Press = '')
$ErrorActionPreference = 'Stop'
# pwsh -File passes "a=1,b=1" as one string; captions here contain no commas.
$Check = @($Check | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
Add-Type -Namespace LodForge -Name S -MemberDefinition '
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, string l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, System.Text.StringBuilder l);'
$p = Get-Process $ProcName | Select-Object -First 1
$wins = $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))
$dlg = $null
foreach ($w in $wins) { if ($w.Current.ClassName -eq $DialogClass) { $dlg = $w; break }; $d = $w.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $DialogClass))); if ($d) { $dlg = $d; break } }
if (-not $dlg) { throw "no $DialogClass" }
function Find([string]$cls) { $dlg.FindAll($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, $cls))) }
foreach ($spec in $Check) {
    $name, $want = $spec -split '=', 2
    $box = Find 'TCheckBox' | Where-Object { $_.Current.Name -eq $name } | Select-Object -First 1
    if (-not $box) { throw "no check box '$name'" }
    $h = [IntPtr]$box.Current.NativeWindowHandle
    $now = [int][LodForge.S]::SendMessage($h, 0xF0, [IntPtr]::Zero, [IntPtr]::Zero)
    if ($now -ne [int]$want) {
        if (-not $box.Current.IsEnabled) { throw "check box '$name' is disabled" }
        $null = [LodForge.S]::SendMessage($h, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero)   # BM_CLICK
        Start-Sleep -Milliseconds 300
    }
    "{0}: {1} -> {2}" -f $name, $now, [int][LodForge.S]::SendMessage($h, 0xF0, [IntPtr]::Zero, [IntPtr]::Zero)
}
foreach ($spec in $Edit) {
    $old, $new = $spec -split '=', 2
    $done = $false
    foreach ($e in Find 'TEdit') {
        $h = [IntPtr]$e.Current.NativeWindowHandle
        $sb = New-Object Text.StringBuilder 2048; $null = [LodForge.S]::SendMessage($h, 0x0D, [IntPtr]2048, $sb)
        if ($sb.ToString().StartsWith($old, [StringComparison]::OrdinalIgnoreCase)) {
            $null = [LodForge.S]::SendMessage($h, 0x0C, [IntPtr]::Zero, $new)          # WM_SETTEXT
            $sb2 = New-Object Text.StringBuilder 2048; $null = [LodForge.S]::SendMessage($h, 0x0D, [IntPtr]2048, $sb2)
            "edit '$($sb.ToString())' -> '$($sb2.ToString())'"; $done = $true; break
        }
    }
    if (-not $done) { throw "no edit starting with '$old'" }
}
if ($Press) {
    $b = Find 'TButton' | Where-Object { $_.Current.Name -eq $Press } | Select-Object -First 1
    if (-not $b) { throw "no button '$Press'" }
    Add-Type -Namespace LodForge -Name P -MemberDefinition '[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);'
    $null = [LodForge.P]::PostMessage([IntPtr]$b.Current.NativeWindowHandle, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero)
    "pressed '$Press'"
}
