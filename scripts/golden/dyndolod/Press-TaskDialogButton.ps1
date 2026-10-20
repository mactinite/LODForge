# Presses a windowless task-dialog button by caption through MSAA (oleacc IAccessible
# accDoDefaultAction) on the tool's #32770 dialog. No mouse/keyboard input, no focus change.
# Run with Windows PowerShell 5.1 (powershell.exe): pwsh 7 rejects the Accessibility reference.
param([string]$ProcName = 'DynDOLODx64', [string]$Name = 'Save and Exit', [int]$WaitSeconds = 180)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Accessibility;
public static class Msaa {
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr hwnd, uint id, ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object obj);
    [DllImport("oleacc.dll")] static extern int AccessibleChildren(IAccessible parent, int start, int count, [Out] object[] children, out int obtained);
    static bool Find(IAccessible a, string name, int depth) {
        if (depth > 12 || a == null) return false;
        int n = 0; try { n = a.accChildCount; } catch { return false; }
        if (n == 0) return false;
        object[] kids = new object[n]; int got;
        AccessibleChildren(a, 0, n, kids, out got);
        for (int i = 0; i < got; i++) {
            IAccessible c = kids[i] as IAccessible;
            if (c != null) {
                string nm = null; try { nm = c.get_accName(0); } catch {}
                if (nm == name) { c.accDoDefaultAction(0); return true; }
                if (Find(c, name, depth + 1)) return true;
            } else {
                string nm = null; try { nm = a.get_accName(kids[i]); } catch {}
                if (nm == name) { a.accDoDefaultAction(kids[i]); return true; }
            }
        }
        return false;
    }
    public static bool Press(IntPtr dialog, string name) {
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71"); object o;
        AccessibleObjectFromWindow(dialog, 0xFFFFFFFC, ref iid, out o);   // OBJID_CLIENT
        return Find((IAccessible)o, name, 0);
    }
}
'@ -ReferencedAssemblies Accessibility
$UIA = [Windows.Automation.AutomationElement]; $Tree = [Windows.Automation.TreeScope]
$p = Get-Process $ProcName | Select-Object -First 1
$dlg = $null
foreach ($w in $UIA::RootElement.FindAll($Tree::Children, (New-Object Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $p.Id)))) {
    $dlg = $w.FindFirst($Tree::Descendants, (New-Object Windows.Automation.PropertyCondition($UIA::ClassNameProperty, '#32770'))); if ($dlg) { break }
}
if (-not $dlg) { throw 'no task dialog open' }
if (-not [Msaa]::Press([IntPtr]$dlg.Current.NativeWindowHandle, $Name)) { throw "no '$Name' in the dialog" }
"pressed '$Name'"
$end = (Get-Date).AddSeconds($WaitSeconds)
while ((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -and (Get-Date) -lt $end) { Start-Sleep 1 }
"process $(if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) { 'still running' } else { 'exited' })"
