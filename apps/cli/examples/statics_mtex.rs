//! Statics `MESH` entries whose model is one of the LOD models in a DetailBake.txt but which still
//! draw it (kept for their AltTextures): counts by AltTextures diffuse.
//! `statics_mtex <LODForgeRuntime dir>`
use std::collections::{BTreeMap, HashSet};

fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let rep = std::fs::read_to_string(dir.join("DetailBake.txt")).unwrap();
    let lods: HashSet<String> = rep.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').next()).map(str::to_string).collect();
    let mut tex: BTreeMap<String, usize> = BTreeMap::new();
    let mut models: BTreeMap<String, usize> = BTreeMap::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if !p.to_string_lossy().ends_with(".statics.bin") {
            continue;
        }
        let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(&p).unwrap()).unwrap();
        for m in &f.meshes {
            if lods.contains(&m.model.to_ascii_lowercase()) {
                let d = m.textures.iter().map(|(d, _)| file_name(d)).collect::<Vec<_>>().join("+");
                *tex.entry(d).or_default() += 1;
                *models.entry(file_name(&m.model)).or_default() += 1;
            }
        }
    }
    let mut t: Vec<_> = tex.into_iter().collect();
    t.sort_by(|a, b| b.1.cmp(&a.1));
    println!("AltTextures diffuses of kept entries:");
    for (k, n) in t.iter().take(25) {
        println!("{n:6}  {k}");
    }
    let mut m: Vec<_> = models.into_iter().collect();
    m.sort_by(|a, b| b.1.cmp(&a.1));
    println!("models:");
    for (k, n) in m.iter().take(12) {
        println!("{n:6}  {k}");
    }
}
