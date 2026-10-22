// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LODForge guided UI. The frontend (dist/) walks through instance -> worldspaces -> options ->
//! output -> build; the backend runs the `lodforge` CLI (`info --json`, `build --json`) and
//! forwards its JSON progress lines to the window as `build-line` events.
//!
//! `lodforge-ui.exe --instance <folder> [--profile <name>]` prefills step 0 and scans at start.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State, Window};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Running build, so it can be cancelled.
#[derive(Default)]
struct Running(Mutex<Option<Child>>);

/// `--instance` / `--profile` from the command line (or from the usvfs relaunch handoff).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
struct LaunchArgs {
    instance: Option<String>,
    profile: Option<String>,
}

impl LaunchArgs {
    /// Accepts `--flag value` and `--flag=value`; anything else is logged and ignored.
    fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut out = Self::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let (flag, inline) = match a.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (a.clone(), None),
            };
            let slot = match flag.as_str() {
                "--instance" => &mut out.instance,
                "--profile" => &mut out.profile,
                _ => {
                    log(&format!("ignoring argument {a:?}"));
                    continue;
                }
            };
            *slot = inline.or_else(|| it.next()).filter(|v| !v.trim().is_empty());
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.instance.is_none() && self.profile.is_none()
    }
}

/// `LODFORGE_UI_LOG=<file>` appends one line per step; the UI has no console.
fn log(msg: &str) {
    if let Ok(p) = std::env::var("LODFORGE_UI_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// Carries the arguments across the explorer.exe relaunch, which cannot pass any:
/// `%LOCALAPPDATA%\LODForge\launch-args.json`, taken once and only while fresh.
fn handoff_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("LODForge").join("launch-args.json"))
}

const HANDOFF_MAX_AGE_SECS: u64 = 60;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
struct Handoff {
    /// Unix seconds when written.
    at: u64,
    args: LaunchArgs,
}

fn write_handoff(args: &LaunchArgs) {
    let Some(p) = handoff_file() else {
        return log("LOCALAPPDATA is not set; the launch arguments are lost");
    };
    let body = serde_json::to_string(&Handoff { at: now_secs(), args: args.clone() }).unwrap_or_default();
    let res = p.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&p, body));
    match res {
        Ok(()) => log(&format!("wrote launch arguments {args:?} to {}", p.display())),
        Err(e) => log(&format!("writing {} failed ({e}); the launch arguments are lost", p.display())),
    }
}

/// Reads and deletes the handoff file; a stale or unreadable one is deleted and ignored.
fn take_handoff() -> LaunchArgs {
    let Some(p) = handoff_file() else { return LaunchArgs::default() };
    let Ok(text) = std::fs::read_to_string(&p) else { return LaunchArgs::default() };
    let _ = std::fs::remove_file(&p);
    match serde_json::from_str::<Handoff>(&text) {
        Ok(h) if now_secs().saturating_sub(h.at) <= HANDOFF_MAX_AGE_SECS => {
            log(&format!("took launch arguments {:?} from {}", h.args, p.display()));
            h.args
        }
        Ok(h) => {
            log(&format!("ignored stale {} ({} s old)", p.display(), now_secs().saturating_sub(h.at)));
            LaunchArgs::default()
        }
        Err(e) => {
            log(&format!("ignored unreadable {} ({e})", p.display()));
            LaunchArgs::default()
        }
    }
}

/// Called by the frontend at start to prefill the instance and profile.
#[tauri::command]
fn launch_args(args: State<'_, LaunchArgs>) -> LaunchArgs {
    args.inner().clone()
}

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
    /// The UI only builds DynDOLOD style (`build --mode dyndolod`); anything but `dyndolod` is refused.
    #[serde(default)]
    mode: Option<String>,
    /// DynDOLOD rule preset (Low, Medium, High).
    #[serde(default)]
    preset: Option<String>,
    /// DynDOLOD.esm/.esp and the DLL NG data files (`--no-plugins` when false).
    #[serde(default = "yes")]
    plugins: bool,
    /// Occlusion.esp (`--no-occlusion` when false).
    #[serde(default = "yes")]
    occlusion: bool,
    /// `pbr` (Community Shaders with PBR) or `standard` (non-PBR: ENB complex material, parallax or
    /// vanilla): `build --materials`.
    #[serde(default)]
    materials: Option<String>,
    /// Grass LOD: `off`, `bto` (grass in object LOD) or `go` (Grass Optimizations meshes).
    #[serde(default)]
    grass: Option<String>,
    /// `auto` or `cache` (with `bto`; both read the NGIO cache).
    #[serde(default)]
    grass_source: Option<String>,
    #[serde(default)]
    grass_density: Option<f32>,
    /// BC7 encoder: `fast`, `high` or `best` (`--bc7`; the CLI default is `high`).
    #[serde(default)]
    bc7: Option<String>,
    /// Terrain texture resampling: `hq` or `xlodgen` (`--terrain-filter`).
    #[serde(default)]
    terrain_filter: Option<String>,
    /// Billboard resolution multiplier (`--billboard-scale`).
    #[serde(default)]
    billboard_scale: Option<f32>,
    /// Object LOD atlas tile cap for LOD textures (`--atlas-max-tile`; full textures get a
    /// quarter, `--atlas-full-max-tile`).
    #[serde(default)]
    atlas_max_tile: Option<u32>,
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
    if let Some(m) = req.mode.as_deref().filter(|m| *m != "dyndolod") {
        return Err(format!("unsupported LOD style {m:?}; the UI builds DynDOLOD style only"));
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
    if let Some(m) = &req.materials {
        c.args(["--materials", m]);
    }
    if let Some(q) = &req.bc7 {
        c.args(["--bc7", q]);
    }
    if let Some(f) = &req.terrain_filter {
        c.args(["--terrain-filter", f]);
    }
    if let Some(k) = req.billboard_scale {
        c.args(["--billboard-scale", &k.to_string()]);
    }
    if let Some(t) = req.atlas_max_tile {
        c.args(["--atlas-max-tile", &t.to_string(), "--atlas-full-max-tile", &(t / 4).max(64).to_string()]);
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

/// The Done screen's "Inspect LODs": opens the viewer (`lodforge view`) on the build's output.
#[derive(Deserialize)]
struct ViewerRequest {
    instance: String,
    profile: String,
    world: String,
    #[serde(default)]
    out: Option<String>,
    #[serde(default)]
    mo2_mod: Option<String>,
}

#[tauri::command]
fn open_viewer(req: ViewerRequest) -> Result<(), String> {
    let out = match (&req.mo2_mod, &req.out) {
        (Some(m), _) => Path::new(&req.instance).join("mods").join(m),
        (None, Some(o)) => PathBuf::from(o),
        _ => return Err("no output folder to inspect".into()),
    };
    if !out.is_dir() {
        return Err(format!("{} is not a folder", out.display()));
    }
    let mut c = command();
    c.args(["view", "--out"]).arg(&out).args(["--world", &req.world, "--instance", &req.instance, "--profile", &req.profile]);
    c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("running {}: {e}", cli().display()))?;
    Ok(())
}

/// The Done screen's "Close LODForge".
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
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
/// Explorer passes no arguments, so `args` cross the relaunch in the handoff file
/// ([`write_handoff`] here, [`take_handoff`] in the restarted UI).
/// The UI has no console; `LODFORGE_UI_LOG=<file>` appends one line per step for diagnosis.
#[cfg(windows)]
fn escape_usvfs(args: &LaunchArgs) {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
    }

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
            if !args.is_empty() {
                write_handoff(args);
            }
            log("starting again outside the virtual file system through explorer.exe");
            match Command::new("explorer.exe").arg(&exe).spawn() {
                Ok(_) => std::process::exit(0),
                Err(e) => {
                    log(&format!("explorer.exe relaunch failed ({e}); continuing hooked"));
                    // This process keeps its own arguments; drop the file so a later start ignores it.
                    if !args.is_empty() {
                        let _ = take_handoff();
                    }
                }
            }
        }
        Err(e) => log(&format!("current_exe failed ({e}); continuing hooked")),
    }
}

fn main() {
    let mut args = LaunchArgs::parse(std::env::args().skip(1));
    if !args.is_empty() {
        log(&format!("launch arguments {args:?}"));
    }
    #[cfg(windows)]
    escape_usvfs(&args);
    if args.is_empty() {
        args = take_handoff();
    }
    tauri::Builder::default()
        .manage(Running::default())
        .manage(args)
        .invoke_handler(tauri::generate_handler![launch_args, list_profiles, mo2_running, profile_info, start_build, build_status, cancel_build, open_viewer, quit])
        .run(tauri::generate_context!())
        .expect("error while running LODForge UI");
}

#[cfg(test)]
mod tests {
    use super::LaunchArgs;

    fn parse(a: &[&str]) -> LaunchArgs {
        LaunchArgs::parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_instance_and_profile() {
        let a = parse(&["--instance", "D:/Modlists/skyrim-cs", "--profile=LODForge"]);
        assert_eq!(a.instance.as_deref(), Some("D:/Modlists/skyrim-cs"));
        assert_eq!(a.profile.as_deref(), Some("LODForge"));
        assert!(parse(&[]).is_empty());
        assert!(parse(&["--instance", "", "--stray"]).is_empty());
    }
}
