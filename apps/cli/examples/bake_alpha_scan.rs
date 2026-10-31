//! Of the models in a DetailBake.txt: which full and LOD models have alpha-tested / blended or
//! effect-shader parts (the bake treats every part as opaque).
//! `bake_alpha_scan <DetailBake.txt> <MO2 instance> [profile]`
use std::path::{Path, PathBuf};

fn roots(instance: &Path, profile: &str) -> Vec<PathBuf> {
    let list = std::fs::read_to_string(instance.join("profiles").join(profile).join("modlist.txt")).unwrap();
    list.lines().filter_map(|l| l.strip_prefix('+')).map(|m| instance.join("mods").join(m.trim())).collect()
}

fn find(roots: &[PathBuf], rel: &str) -> Option<Vec<u8>> {
    roots.iter().map(|r| r.join(rel)).find(|p| p.is_file()).and_then(|p| std::fs::read(p).ok())
}

fn alpha_parts(roots: &[PathBuf], rel: &str) -> Option<(usize, Vec<String>)> {
    let m = lodforge_objlod::mesh::import(rel, &find(roots, rel)?).ok()?;
    let mut names = Vec::new();
    for p in &m.parts {
        let effect = matches!(p.shader, lodforge_objlod::mesh::ShaderSource::Effect(_));
        let alpha = p.alpha.is_some_and(|(f, _)| f & 0x201 != 0);
        if effect || alpha {
            names.push(format!("{}{} ({})", if effect { "effect " } else { "" }, p.name, p.textures[0]));
        }
    }
    Some((m.parts.len(), names))
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let roots = roots(Path::new(&a[1]), a.get(2).map_or("Default", |s| s.as_str()));
    let text = std::fs::read_to_string(&a[0]).unwrap();
    let (mut full_n, mut lod_n, mut total) = (0, 0, 0);
    let mut seen = std::collections::HashSet::new();
    for l in text.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 3 {
            continue;
        }
        total += 1;
        if let Some((_, names)) = alpha_parts(&roots, c[0])
            && !names.is_empty()
        {
            lod_n += 1;
            println!("LOD  {}: {}", c[0], names.join("; "));
        }
        if seen.insert(c[1].to_string())
            && let Some((_, names)) = alpha_parts(&roots, c[1])
            && !names.is_empty()
        {
            full_n += 1;
            println!("FULL {}: {}", c[1], names.join("; "));
        }
    }
    println!("{total} baked LOD models: {lod_n} with alpha/effect parts; {full_n} full models with alpha/effect parts");
}
