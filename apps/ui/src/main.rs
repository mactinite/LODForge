// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LODForge guided UI. The frontend (dist/) walks through instance -> worldspaces -> options ->
//! output -> build; the backend runs the `lodforge` CLI (`info --json`, `build --json`) and
//! forwards its JSON progress lines to the window as `build-line` events.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::Deserialize;
use tauri::{Emitter, State, Window};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Running build, so it can be cancelled.
#[derive(Default)]
struct Running(Mutex<Option<Child>>);

/// The CLI next to this executable (same cargo target folder), or `LODFORGE_CLI`.
fn cli() -> PathBuf {
    if let Ok(p) = std::env::var("LODFORGE_CLI") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name("lodforge.exe")
}

fn command() -> Command {
    let mut c = Command::new(cli());
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[tauri::command]
fn list_profiles(instance: String) -> Result<Vec<String>, String> {
    let dir = Path::new(&instance).join("profiles");
    let mut out: Vec<String> = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    out.sort();
    Ok(out)
}

#[tauri::command]
fn mo2_running() -> bool {
    Command::new("tasklist").args(["/FI", "IMAGENAME eq ModOrganizer.exe", "/NH"]).creation_flags_safe().output().map(|o| String::from_utf8_lossy(&o.stdout).contains("ModOrganizer.exe")).unwrap_or(false)
}

trait NoWindow {
    fn creation_flags_safe(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn creation_flags_safe(&mut self) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(0x0800_0000);
        self
    }
}

#[tauri::command]
async fn profile_info(instance: String, profile: String) -> Result<serde_json::Value, String> {
    let out = command().args(["info", "--json", "--instance", &instance, "--profile", &profile]).output().map_err(|e| format!("running {}: {e}", cli().display()))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("no JSON from lodforge info")?;
    serde_json::from_str(line).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct BuildRequest {
    instance: String,
    profile: String,
    worlds: Vec<String>,
    seasons: Vec<String>,
    terrain: bool,
    objects: bool,
    trees: bool,
    /// Render tree billboards first (`build --texgen`) instead of relying on installed ones.
    #[serde(default)]
    texgen: bool,
    /// `xlodgen` (default) or `dyndolod` (`build --mode`).
    #[serde(default)]
    mode: Option<String>,
    /// DynDOLOD rule preset (Low, Medium, High).
    #[serde(default)]
    preset: Option<String>,
    /// DynDOLOD mode: DynDOLOD.esm/.esp and the DLL NG data files.
    #[serde(default = "yes")]
    plugins: bool,
    /// DynDOLOD mode: Occlusion.esp.
    #[serde(default = "yes")]
    occlusion: bool,
    /// Grass LOD: `off`, `bto` (DynDOLOD mode) or `go` (Grass Optimizations meshes).
    #[serde(default)]
    grass: Option<String>,
    /// `auto` or `cache` (with `bto`; both read the NGIO cache).
    #[serde(default)]
    grass_source: Option<String>,
    #[serde(default)]
    grass_density: Option<f32>,
    mo2_mod: Option<String>,
    out: Option<String>,
    enable: bool,
}

fn yes() -> bool {
    true
}

#[tauri::command]
fn start_build(window: Window, running: State<'_, Running>, req: BuildRequest) -> Result<(), String> {
    let mut guard = running.0.lock().unwrap();
    if guard.is_some() {
        return Err("a build is already running".into());
    }
    let mut c = command();
    c.args(["build", "--json", "--instance", &req.instance, "--profile", &req.profile]);
    if !req.worlds.is_empty() {
        c.args(["--worlds", &req.worlds.join(",")]);
    }
    if !req.seasons.is_empty() {
        c.args(["--seasons", &req.seasons.join(",")]);
    }
    for (on, flag) in [(req.terrain, "--no-terrain"), (req.objects, "--no-objects"), (req.trees, "--no-trees")] {
        if !on {
            c.arg(flag);
        }
    }
    if req.texgen {
        c.arg("--texgen");
    }
    if req.mode.as_deref() == Some("dyndolod") {
        c.args(["--mode", "dyndolod"]);
        if let Some(preset) = &req.preset {
            c.args(["--preset", preset]);
        }
        if !req.plugins {
            c.arg("--no-plugins");
        }
        if !req.occlusion {
            c.arg("--no-occlusion");
        }
    }
    if let Some(g) = req.grass.as_deref().filter(|g| *g != "off") {
        c.args(["--grass", g]);
        if g == "bto" {
            if let Some(src) = &req.grass_source {
                c.args(["--grass-source", src]);
            }
            if let Some(d) = req.grass_density {
                c.args(["--grass-density", &d.to_string()]);
            }
        }
    }
    match (&req.mo2_mod, &req.out) {
        (Some(m), _) => {
            c.args(["--mo2-mod", m]);
            if req.enable {
                c.arg("--enable");
            }
        }
        (None, Some(o)) => {
            c.args(["--out", o]);
        }
        _ => return Err("choose an MO2 mod name or an output folder".into()),
    }
    let mut child = c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("running {}: {e}", cli().display()))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    *guard = Some(child);
    let w = window.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = w.emit("build-line", line);
        }
    });
    let w = window.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = w.emit("build-error", line);
        }
    });
    Ok(())
}

/// Polled by the frontend: Some(exit code) once the build process has exited.
#[tauri::command]
fn build_status(running: State<'_, Running>) -> Option<i32> {
    let mut guard = running.0.lock().unwrap();
    let done = match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
            Ok(None) => None,
            Err(_) => Some(-1),
        },
        None => return Some(-2),
    };
    if done.is_some() {
        *guard = None;
    }
    done
}

#[tauri::command]
fn cancel_build(running: State<'_, Running>) {
    if let Some(mut c) = running.0.lock().unwrap().take() {
        let _ = c.kill();
    }
}

/// Mod Organizer 2 starts its tools inside the usvfs virtual file system and injects its hook DLL
/// into every process they spawn. WebView2's helper processes (`msedgewebview2.exe`) do not
/// survive that injection, and the UI dies with them a few seconds after launch. LODForge reads
/// the MO2 instance folders directly and needs nothing from the VFS, so when this process finds
/// the hook DLL loaded it starts itself again through the desktop shell (`explorer.exe` hands the
/// launch to the already-running, unhooked shell process) and exits. The restarted UI runs
/// outside the VFS; the MO2 tool entry can point at `lodforge-ui.exe` as it would for any tool.
///
/// Verified on MO2 2.5.2 / usvfs 0.5.6.1 (2026-10-20). The alternative, calling usvfs's exported
/// `usvfsBlacklistExecutable` for the WebView2 runtime from inside the hooked process, faults
/// inside `usvfs_x64.dll`: that export is for the MO2 process that owns the parameters, not for
/// injected children. Adding `msedgewebview2.exe` to MO2's executables blacklist setting works
/// too, but has to be carried by every instance.
///
/// The UI has no console; `LODFORGE_UI_LOG=<file>` appends one line per step for diagnosis.
#[cfg(windows)]
fn escape_usvfs() {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
    }

    let log = |msg: &str| {
        if let Ok(p) = std::env::var("LODFORGE_UI_LOG") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{msg}");
            }
        }
    };

    let dll: Vec<u16> = "usvfs_x64.dll".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated UTF-16 module name; a null result means the DLL is not loaded.
    if unsafe { GetModuleHandleW(dll.as_ptr()) }.is_null() {
        return;
    }
    log("usvfs_x64.dll is loaded: running inside MO2's virtual file system");
    if std::env::var_os("LODFORGE_UI_STAY_HOOKED").is_some() {
        log("LODFORGE_UI_STAY_HOOKED set; continuing inside the VFS");
        return;
    }
    match std::env::current_exe() {
        Ok(exe) => {
            log("starting again outside the virtual file system through explorer.exe");
            match Command::new("explorer.exe").arg(&exe).spawn() {
                Ok(_) => std::process::exit(0),
                Err(e) => log(&format!("explorer.exe relaunch failed ({e}); continuing hooked")),
            }
        }
        Err(e) => log(&format!("current_exe failed ({e}); continuing hooked")),
    }
}

fn main() {
    #[cfg(windows)]
    escape_usvfs();
    tauri::Builder::default()
        .manage(Running::default())
        .invoke_handler(tauri::generate_handler![list_profiles, mo2_running, profile_info, start_build, build_status, cancel_build])
        .run(tauri::generate_context!())
        .expect("error while running LODForge UI");
}
