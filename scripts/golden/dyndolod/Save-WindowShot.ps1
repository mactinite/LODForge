param([string]$ProcName = "DynDOLODx64", [long]$Hwnd = 0, [string]$Out = (Join-Path $env:TEMP 'lodforge-shot.png'))
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace LodForge -Name W5 -MemberDefinition '
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
public struct RECT { public int L, T, R, B; }'
$p = Get-Process $ProcName | Select-Object -First 1
$h = if ($Hwnd) { [IntPtr]$Hwnd } else { $p.MainWindowHandle }
$r = New-Object LodForge.W5+RECT
$null = [LodForge.W5]::GetWindowRect($h, [ref]$r)
$bmp = New-Object Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc(); $null = [LodForge.W5]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc)
$bmp.Save($Out)
"$($r.R - $r.L)x$($r.B - $r.T)"
