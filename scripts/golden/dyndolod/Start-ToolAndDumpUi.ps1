param([string]$Exe = 'TexGen x64', [string]$ProcName = 'TexGenx64', [string]$OutDir = 'D:\Modlists\LODForge-golden\probe', [string]$Dump = (Join-Path $env:TEMP 'lodforge-ui-dump.txt'), [int]$Wait = 600, [string]$Mo2Profile = 'LODForge-DynDOLOD', [string]$Instance = 'D:Modlistsskyrim-cs')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
$inst = $Instance
New-Item -ItemType Directory -Force $OutDir | Out-Null
$toolArgs = "-sse -d:\`"$inst\Stock Game\Data\`" -o:\`"$($OutDir.TrimEnd('\'))\`""   # quoted: mod folders have spaces
Start-Process "$inst\ModOrganizer.exe" -ArgumentList "-p `"$Mo2Profile`" run -e -a `"$toolArgs`" `"$Exe`""
$deadline = (Get-Date).AddSeconds(120)
while (-not ($p = Get-Process $ProcName -ErrorAction SilentlyContinue | Select-Object -First 1)) { if ((Get-Date) -gt $deadline) { throw 'no process' }; Start-Sleep -Milliseconds 500 }
"pid $($p.Id)" | Set-Content $Dump
$t0 = Get-Date
# Dump all top-level windows of the process every 10 s, with descendants, until $Wait seconds.
$seen = @{}
while (((Get-Date) - $t0).TotalSeconds -lt $Wait -and -not $p.HasExited) {
    $wins = $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))
    foreach ($w in $wins) {
        $all = @($w) + @($w.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition))
        $sig = ($all | ForEach-Object { $_.Current.ClassName + ':' + $_.Current.Name }) -join '|'
        if ($seen.ContainsKey($sig)) { continue }
        $seen[$sig] = 1
        "=== t=$([int]((Get-Date)-$t0).TotalSeconds)s top '$($w.Current.Name)' [$($w.Current.ClassName)]" | Add-Content $Dump
        foreach ($e in $all) {
            $r = $e.Current.BoundingRectangle
            '{0} | {1} | hwnd={2} | {3},{4} {5}x{6} | en={7}' -f $e.Current.ClassName, $e.Current.Name, $e.Current.NativeWindowHandle, [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height, $e.Current.IsEnabled | Add-Content $Dump
        }
    }
    Start-Sleep -Seconds 10
}
"done exited=$($p.HasExited)" | Add-Content $Dump
