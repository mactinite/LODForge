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
async fn profile_info(current: State<'_, Current>, instance: String, profile: String) -> Result<serde_json::Value, String> {
    *current.0.lock().unwrap() = Some(PathBuf::from(&instance));
    let out = command().args(["info", "--json", "--instance", &instance, "--profile", &profile]).output().map_err(|e| format!("running {}: {e}", cli().display()))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("no JSON from lodforge info")?;
    serde_json::from_str(line).map_err(|e| e.to_string())
}

/// The flags `lodforge build --help` lists, read once: `start_build` refuses anything else.
fn build_flags() -> Result<&'static std::collections::HashSet<String>, String> {
    static FLAGS: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    if let Some(f) = FLAGS.get() {
        return Ok(f);
    }
    let out = command().args(["build", "--help"]).output().map_err(|e| format!("running {}: {e}", cli().display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut set = std::collections::HashSet::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if word.len() > 2 && word.starts_with("--") {
            set.insert(word.to_string());
        }
    }
    if set.is_empty() {
        return Err("lodforge build --help listed no flags".into());
    }
    Ok(FLAGS.get_or_init(|| set))
}

/// Checks a `build` command line from the frontend: `build` first, every `--flag` one the CLI
/// knows, DynDOLOD style only.
fn check_build_args(args: &[String], known: &std::collections::HashSet<String>) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("build") {
        return Err("the command must start with `build`".into());
    }
    for (i, a) in args.iter().enumerate().skip(1) {
        if a.starts_with("--") {
            let flag = a.split_once('=').map_or(a.as_str(), |(f, _)| f);
            if !known.contains(flag) {
                return Err(format!("unknown build option {flag}"));
            }
            if flag == "--mode" && args.get(i + 1).map(String::as_str) != Some("dyndolod") {
                return Err("the UI builds DynDOLOD style only (--mode dyndolod)".into());
            }
        }
    }
    Ok(())
}

/// Runs `lodforge <args>` (a `build --json ...` command line from the frontend's option catalog)
/// and forwards its output lines as `build-line` / `build-error` events.
#[tauri::command]
fn start_build(window: Window, running: State<'_, Running>, args: Vec<String>) -> Result<(), String> {
    let mut guard = running.0.lock().unwrap();
    if guard.is_some() {
        return Err("a build is already running".into());
    }
    check_build_args(&args, build_flags()?)?;
    log(&format!("build {args:?}"));
    let mut c = command();
    c.args(&args);
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

/// The instance the frontend last scanned (`profile_info`); config files are read from it.
#[derive(Default)]
struct Current(Mutex<Option<PathBuf>>);

const OVERLAY_MOD: &str = "LODForge Config";

fn instance_of(current: &State<'_, Current>) -> Result<PathBuf, String> {
    current.0.lock().unwrap().clone().ok_or_else(|| "scan an instance first".to_string())
}

/// DynDOLOD's `Edit Scripts\DynDOLOD` folder in the instance.
fn dyndolod_dir(instance: &Path) -> PathBuf {
    instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD")
}

/// The overlay folder the CLI reads first (lodforge_dyndolod::overlay).
fn overlay_dir(instance: &Path) -> PathBuf {
    instance.join("mods").join(OVERLAY_MOD).join("DynDOLOD")
}

/// Format id for the frontend's table editors (configFormats.ts), from the file name.
fn config_format(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.starts_with("rules\\") {
        "rules"
    } else if n.contains("mod_world_ignore") {
        "mod_world_ignore"
    } else if n.contains("childworldfull") || n.contains("xesplod") {
        "plugin_formid"
    } else if n.contains("childworldmatches") {
        "childworld_matches"
    } else if n.contains("mesh_crc32") {
        "mesh_crc"
    } else if n.contains("worldspace_ignore") {
        "plugin_formid"
    } else if n.contains("misnamed") || n.contains("default_lod") {
        "path_list"
    } else if n.contains("mesh_replace") {
        "mesh_replace"
    } else if n.contains("texgen_copy") {
        "texture_copy"
    } else if n.contains("texgen_billboards") || n.contains("required_billboards") {
        "texgen_billboards"
    } else if n.ends_with(".ini") {
        "ini"
    } else {
        "text"
    }
}

/// A config name from the frontend: a relative path below the DynDOLOD folder, in `Configs` or
/// `Rules` (or DynDOLOD_SSE.ini), with no `..`.
fn check_config_name(name: &str) -> Result<(), String> {
    let n = name.replace('/', "\\");
    let lower = n.to_ascii_lowercase();
    let ok_dir = lower.starts_with("configs\\") || lower.starts_with("rules\\") || lower == "dyndolod_sse.ini";
    let ok_ext = lower.ends_with(".txt") || lower.ends_with(".ini");
    if !ok_dir || !ok_ext || n.split('\\').any(|p| p == ".." || p.is_empty()) || n.contains(':') {
        return Err(format!("not a DynDOLOD config file: {name}"));
    }
    Ok(())
}

/// Whether the LODForge build reads this file (the others are DynDOLOD/TexGen-only).
fn config_used(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("rules\\")
        || n == "dyndolod_sse.ini"
        || ["mod_world_ignore", "childworldfull", "childworld_", "childworldmatches", "mesh_crc32", "xesplod", "misnamed", "flat_normals"].iter().any(|k| n.contains(&format!("dyndolod_sse_{k}")))
}

#[derive(Serialize)]
struct ConfigFile {
    name: String,
    format: &'static str,
    source: &'static str,
    size: u64,
    used: bool,
}

/// Every config file LODForge reads: DynDOLOD's `Configs\*.txt|ini`, `Rules\*.ini` and
/// DynDOLOD_SSE.ini, merged with the overlay's copies (case-insensitive names).
#[tauri::command]
fn config_files(current: State<'_, Current>) -> Result<Vec<ConfigFile>, String> {
    let instance = instance_of(&current)?;
    let (dd, ov) = (dyndolod_dir(&instance), overlay_dir(&instance));
    let mut by_name: std::collections::BTreeMap<String, ConfigFile> = std::collections::BTreeMap::new();
    for (root, source) in [(&dd, "dyndolod"), (&ov, "overlay")] {
        let mut add = |rel: String, path: &Path| {
            // Only Skyrim SE's files (DynDOLOD also ships Enderal, Fallout 4 and TES5 ones).
            let file = rel.rsplit('\\').next().unwrap_or(&rel).to_ascii_lowercase();
            if check_config_name(&rel).is_err() || !file.starts_with("dyndolod_sse") {
                return;
            }
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            by_name.insert(rel.to_ascii_lowercase(), ConfigFile { format: config_format(&rel), used: config_used(&rel), name: rel, source, size });
        };
        for sub in ["Configs", "Rules"] {
            if let Ok(rd) = std::fs::read_dir(root.join(sub)) {
                for e in rd.flatten().filter(|e| e.path().is_file()) {
                    add(format!("{sub}\\{}", e.file_name().to_string_lossy()), &e.path());
                }
            }
        }
        let ini = root.join("DynDOLOD_SSE.ini");
        if ini.is_file() {
            add("DynDOLOD_SSE.ini".into(), &ini);
        }
    }
    if by_name.is_empty() {
        return Err(format!("no DynDOLOD config files in {}", dd.display()));
    }
    Ok(by_name.into_values().collect())
}

/// The file in use for `name`: the overlay's copy, else DynDOLOD's (case-insensitive lookup).
fn resolve_config(instance: &Path, name: &str) -> Option<PathBuf> {
    for root in [overlay_dir(instance), dyndolod_dir(instance)] {
        let p = root.join(name);
        if p.is_file() {
            return Some(p);
        }
        let (dir, file) = match name.rsplit_once('\\') {
            Some((d, f)) => (root.join(d), f),
            None => (root.clone(), name),
        };
        if let Ok(rd) = std::fs::read_dir(dir) {
            if let Some(e) = rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(file)) {
                return Some(e.path());
            }
        }
    }
    None
}

/// Config text, decoded as UTF-8 or else Latin-1 (DynDOLOD's files are ANSI).
#[tauri::command]
fn read_config(current: State<'_, Current>, name: String) -> Result<String, String> {
    check_config_name(&name)?;
    let instance = instance_of(&current)?;
    let p = resolve_config(&instance, &name.replace('/', "\\")).ok_or_else(|| format!("{name} not found"))?;
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_string(),
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// Saves an edited copy into the LODForge Config mod (created with a meta.ini on first save).
#[tauri::command]
fn write_config(current: State<'_, Current>, name: String, text: String) -> Result<(), String> {
    check_config_name(&name)?;
    let instance = instance_of(&current)?;
    let p = overlay_dir(&instance).join(name.replace('/', "\\"));
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", p.display()))?;
    let meta = instance.join("mods").join(OVERLAY_MOD).join("meta.ini");
    if !meta.exists() {
        let _ = std::fs::write(&meta, "[General]\r\nmodid=0\r\nversion=\r\ncomments=Edited DynDOLOD config files from the LODForge UI. LODForge reads these first; DynDOLOD itself does not.\r\n");
    }
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    log(&format!("saved {}", p.display()));
    Ok(())
}

/// Deletes the overlay's copy, so DynDOLOD's file is used again.
#[tauri::command]
fn reset_config(current: State<'_, Current>, name: String) -> Result<(), String> {
    check_config_name(&name)?;
    let instance = instance_of(&current)?;
    let p = overlay_dir(&instance).join(name.replace('/', "\\"));
    if p.is_file() {
        std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

/// A report the build wrote into its output (`LODForge_TreeReport_<World>.csv`).
#[tauri::command]
fn read_report(instance: String, out: Option<String>, mo2_mod: Option<String>, name: String) -> Result<String, String> {
    if name.contains(['\\', '/', ':']) || !name.starts_with("LODForge_") {
        return Err(format!("not a LODForge report: {name}"));
    }
    let dir = match (&mo2_mod, &out) {
        (Some(m), _) => Path::new(&instance).join("mods").join(m),
        (None, Some(o)) => PathBuf::from(o),
        _ => return Err("no output folder".into()),
    };
    let p = dir.join(&name);
    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
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
        .manage(Current::default())
        .manage(args)
        .invoke_handler(tauri::generate_handler![launch_args, list_profiles, mo2_running, profile_info, start_build, build_status, cancel_build, open_viewer, quit, config_files, read_config, write_config, reset_config, read_report])
        .run(tauri::generate_context!())
        .expect("error while running LODForge UI");
}

#[cfg(test)]
mod tests {
    use super::{check_build_args, check_config_name, config_format, LaunchArgs};

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

    #[test]
    fn build_args_and_config_names() {
        let known: std::collections::HashSet<String> = ["--json", "--mode", "--instance"].iter().map(|s| s.to_string()).collect();
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_build_args(&a(&["build", "--json", "--mode", "dyndolod", "--instance", "D:/x"]), &known).is_ok());
        assert!(check_build_args(&a(&["info", "--json"]), &known).is_err());
        assert!(check_build_args(&a(&["build", "--evil"]), &known).is_err());
        assert!(check_build_args(&a(&["build", "--mode", "xlodgen"]), &known).is_err());
        assert!(check_config_name(r"Configs\DynDOLOD_SSE_mod_world_ignore.txt").is_ok());
        assert!(check_config_name("DynDOLOD_SSE.ini").is_ok());
        assert!(check_config_name(r"Configs\..\..\x.txt").is_err());
        assert!(check_config_name(r"C:\x.ini").is_err());
        assert_eq!(config_format(r"Rules\DynDOLOD_SSE_High.ini"), "rules");
        assert_eq!(config_format(r"Configs\DynDOLOD_SSE_xesplod.txt"), "plugin_formid");
        assert!(super::config_used(r"Configs\DynDOLOD_SSE_childworld_MarkarthWorld.ini"));
        assert!(super::config_used(r"Configs\DynDOLOD_SSE_ChildworldMatches.txt"));
        assert!(!super::config_used(r"Configs\DynDOLOD_SSE_TexGen_copy_skyrimesm.txt"));
    }
}
