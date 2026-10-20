//! In-place rebuild determinism: `lodforge build` run twice with the same arguments into the
//! same output and work folders must write the same bytes the second time. Opt-in (it needs an
//! MO2 instance and the GPU):
//!
//! ```text
//! set LODFORGE_REBUILD_ROOT=D:\scratch\rebuild
//! set LODFORGE_REBUILD_ARGS=--instance D:\Modlists\skyrim-cs --profile LODForge-E2E --mode dyndolod --preset Medium --texgen --worlds JaphetsFollyWorld
//! cargo test --release -p lodforge --test rebuild -- --ignored
//! ```
//!
//! `LODFORGE_REBUILD_ARGS` is split on whitespace (so no mod names with spaces: leave earlier
//! LOD outputs disabled in the profile or rely on the default exclusion of generated LOD mods);
//! `--out`, `--work-dir` and `--json` are added here. The root must not exist yet. Run it with
//! `--release`: the test drives the `lodforge` binary of the same profile, and a debug build is
//! far too slow for TexGen and LODGen. A small worldspace takes about a minute per run, the
//! Tamriel full-validation command (docs/validation-20261019.md) about three.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

fn tree_digest(root: &Path) -> BTreeMap<String, (u64, u64)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                let mut h = DefaultHasher::new();
                bytes.hash(&mut h);
                out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('/', "\\").to_ascii_lowercase(), (bytes.len() as u64, h.finish()));
            }
        }
    }
    out
}

fn build(args: &[String], out: &Path, work: &Path, log: &Path) {
    let exe = env!("CARGO_BIN_EXE_lodforge");
    let status = std::process::Command::new(exe)
        .arg("build")
        .args(args)
        .args(["--out", &out.to_string_lossy(), "--work-dir", &work.to_string_lossy(), "--json"])
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err.log")).unwrap())
        .status()
        .unwrap();
    assert!(status.success(), "build failed (see {})", log.display());
}

#[test]
#[ignore = "requires an MO2 instance and the GPU: LODFORGE_REBUILD_ROOT (new folder) and LODFORGE_REBUILD_ARGS"]
fn in_place_rebuild_is_byte_identical() {
    let root = PathBuf::from(std::env::var_os("LODFORGE_REBUILD_ROOT").expect("LODFORGE_REBUILD_ROOT"));
    let args: Vec<String> = std::env::var("LODFORGE_REBUILD_ARGS").expect("LODFORGE_REBUILD_ARGS").split_whitespace().map(str::to_string).collect();
    assert!(!root.exists(), "{} must be a new folder", root.display());
    std::fs::create_dir_all(&root).unwrap();
    let (out, work) = (root.join("Data"), root.join("work"));
    build(&args, &out, &work, &root.join("build-1.log"));
    let first = tree_digest(&out);
    assert!(!first.is_empty(), "the first build wrote nothing");
    build(&args, &out, &work, &root.join("build-2.log"));
    let second = tree_digest(&out);
    let changed: Vec<&String> = first.iter().filter(|(k, v)| second.get(*k) != Some(v)).map(|(k, _)| k).collect();
    let added: Vec<&String> = second.keys().filter(|k| !first.contains_key(*k)).collect();
    assert!(changed.is_empty() && added.is_empty(), "{} of {} files changed in place, {} added: {:?} {:?}", changed.len(), first.len(), added.len(), changed.iter().take(20).collect::<Vec<_>>(), added.iter().take(20).collect::<Vec<_>>());
}
