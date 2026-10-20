# DynDOLOD / TexGen golden-run harness

These scripts drove the P6 golden runs (docs/baseline.md, "P6"). Both tools are Delphi VCL apps:
UIA is only used to find windows, and everything else goes through Win32 messages. Close MO2 and
the tools before starting, and never run this while the user has them open.

| Script | Purpose |
|---|---|
| `Start-ToolAndDumpUi.ps1 -Exe "DynDOLOD x64" -ProcName DynDOLODx64 -OutDir <dir>` | Launches an MO2 executable entry in profile `LODForge-DynDOLOD` with `-sse -d:<Stock Game\Data> -o:<dir>`, then dumps every window and control (class, name, hwnd) every 10 s. |
| `Invoke-ToolRun.ps1 -ProcName TexGenx64 -DialogClass TfrmTexGen -Button Start -RunDir <dir> -DoneRegex 'completed successfully\|Errors have occured'` | Records the options dialog (checkbox states, combo/edit texts → `options.json`), clicks the button and samples the memo log every 5 s into `timeline.csv` until it matches. DynDOLOD: `-DialogClass TfrmDynDOLODAdvanced -Button OK`. |
| `Click-Button.ps1 -ProcName <p> -Name "Exit TexGen"` | Synchronous `BM_CLICK`. TaskDialog buttons (`CCPushButton`) ignore `PostMessage`. |
| `Set-Worldspaces.ps1 -List <hwnd> -Keep Tamriel -Apply` | Lists the worldspace `TCheckListBox` and toggles every item except `Keep` (select + space). All items start checked. |
| `Save-WindowShot.ps1 -Hwnd <hwnd>` | PrintWindow capture, for checking dialog state. |
| `Invoke-QuickAutoClean.ps1 -Plugins X.esm` | SSEEdit QuickAutoClean of one plugin through MO2. It closes the donation nag. **It writes into Stock Game\Data in place.** Pass one plugin per call. |

Added for the LODForge-E2E reference run (2026-10-18):

| Script | Purpose |
|---|---|
| `Get-DialogControls.ps1 -ProcName <p> -DialogClass <cls>` | Waits for a dialog and lists its controls with check states. |
| `Set-DialogOptions.ps1 -Check 'Seasons=1,Grass LOD=1' -Edit '<old path prefix>=<new path>' -Press 'Medium'` | Sets check boxes with real clicks, edit boxes with WM_SETTEXT, then presses a TButton. |
| `Get-ToolLog.ps1 -ProcName <p>` | Last lines of the tool's log memo plus any open task dialog's text and buttons. |
| `Wait-ToolDone.ps1 -ProcName <p>` | Waits for "completed successfully" or a task dialog; ignores DynDOLOD's non-fatal `<Error: Deleted reference ...>` lines (Invoke-ToolRun's regex stops on them). |
| `Close-TaskDialog.ps1`, `Press-TaskDialogButton.ps1` | Attempts to press task-dialog buttons remotely. See the warning below. |

Lessons: TexGen and DynDOLOD remember the last run's options, worldspace selection and output path
in their dialogs (the `-o:` switch is overridden), so set them explicitly. DynDOLOD refuses an output
folder inside MO2's `mods` ("Path not allowed"); write elsewhere and move the result into a mod.
DynDOLOD also refuses to start with `SSE-Terrain-Tamriel-Extend.esm` active or with "DynDOLOD" lines
in a seasonal swap INI.

**Warning, task-dialog buttons:** DynDOLOD's end-of-run buttons are windowless DirectUI elements (no
hwnd, control id 0). `TDM_CLICK_BUTTON` with id 0 dismisses the dialog without a choice; then
closing the window exits without saving the plugins (a run lost DynDOLOD.esm/.esp/Occlusion.esp
that way). UIA InvokePattern is unsupported and MSAA `accDoDefaultAction` reports success without
pressing. Until that is solved, have the user click "Save and Exit".

Task dialogs (TexGen "Ignore"/"Exit TexGen") sometimes stay open after `Click-Button.ps1`. Then
also send `TDM_CLICK_BUTTON` (0x466) to the `#32770` dialog; the combination closes it.

Grass golden (2026-10-18): set `CSGrassOptimizations=-1` in `Edit Scripts\DynDOLOD\TexGen_SSE.ini`
and `DynDOLOD_SSE.ini` first (back them up and restore them afterwards). Otherwise Community
Shaders replaces the Grass LOD checkbox with CS Optimized Grass assets. TexGen: HD Grass only.
Install the output as a mod in `LODForge-DynDOLOD`, then run DynDOLOD with "Grass LOD" checked.
At the end, DynDOLOD opens its summary in the default browser, and MO2 waits for it ("Exit Now").

DynDOLOD flow: start the tool, Advanced >>>, Medium, keep Tamriel only, then `Invoke-ToolRun`.
At the end, click "Save and Exit". Copy `Edit Scripts\Export`, `Logs`,
`Presets\DynDOLOD_SSE_Default.ini` and the output folder into the golden directory.
