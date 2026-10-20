param([long]$List, [string]$Keep = '', [switch]$Apply)
Add-Type -Namespace LodForge -Name W6 -MemberDefinition '
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, int m, IntPtr w, System.Text.StringBuilder l);'
$h = [IntPtr]$List
$n = [int][LodForge.W6]::SendMessage($h, 0x018B, [IntPtr]::Zero, [IntPtr]::Zero)
for ($i = 0; $i -lt $n; $i++) {
    $len = [int][LodForge.W6]::SendMessage($h, 0x018A, [IntPtr]$i, [IntPtr]::Zero)
    $sb = New-Object Text.StringBuilder ([Math]::Max($len + 2, 4))
    $null = [LodForge.W6]::SendMessage($h, 0x0189, [IntPtr]$i, $sb)
    $name = $sb.ToString()
    "$i [$len] $name"
    if ($Apply) {
        $want = $name -like "$Keep (*"
        # Toggle by selecting the item and sending a space character (TCheckListBox KeyPress).
        # Caller supplies the current state through two passes: first uncheck all via toggles is unsafe,
        # so we just toggle items whose desired state differs from checked-by-default.
        if (-not $want) {
            $null = [LodForge.W6]::SendMessage($h, 0x0186, [IntPtr]$i, [IntPtr]::Zero)
            $null = [LodForge.W6]::SendMessage($h, 0x0102, [IntPtr]32, [IntPtr]::Zero)
        }
    }
}
