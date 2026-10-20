//! Texture and shape statistics of the object LOD files of one world and level: distinct first
//! textures, their references, and shape counts by name. Season files (`_SPR` etc.) are skipped.
//! `cargo run -p lodforge-nif --example bto_stats -- <dir with .bto> <World> <level> [other dir]`
//! With a second directory, also lists the textures only one side references (top 15 each).

use std::collections::BTreeMap;
use std::path::Path;

use lodforge_nif::{Block, parse};

#[derive(Default)]
struct Stats {
    files: usize,
    textures: BTreeMap<String, usize>,
    shapes: BTreeMap<String, usize>,
}

fn stats(dir: &Path, world: &str, level: &str) -> Stats {
    let prefix = format!("{}.{level}.", world.to_lowercase());
    let mut s = Stats::default();
    for e in std::fs::read_dir(dir).expect("dir").flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !name.starts_with(&prefix) || !name.ends_with(".bto") || name.contains('_') {
            continue;
        }
        let Ok(nif) = parse(&std::fs::read(e.path()).unwrap()) else { continue };
        s.files += 1;
        for b in &nif.blocks {
            let Block::TriShape(t) = b else { continue };
            let shape = t.av.net.name.clone().unwrap_or_default();
            // obj, obj-LargeRef, objHD, ... (the name without its running number).
            let kind = shape.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':').to_string();
            *s.shapes.entry(kind).or_default() += 1;
            let tex = match nif.get(t.shader_property) {
                Some(Block::LightingShader(l)) => match nif.get(l.texture_set) {
                    Some(Block::TextureSet(ts)) => ts.first().cloned().unwrap_or_default(),
                    _ => String::new(),
                },
                Some(Block::EffectShader(e)) => e.source_texture.clone(),
                _ => String::new(),
            };
            *s.textures.entry(tex.to_lowercase()).or_default() += 1;
        }
    }
    s
}

fn print(label: &str, s: &Stats) {
    let refs: usize = s.textures.values().sum();
    println!("{label}: {} files, {} distinct textures, {} references", s.files, s.textures.len(), refs);
    for (k, n) in &s.shapes {
        println!("  shapes {k:<20} {n}");
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a.len() >= 3, "usage: bto_stats <dir> <World> <level> [other dir]");
    let ours = stats(Path::new(&a[0]), &a[1], &a[2]);
    print(&a[0], &ours);
    if let Some(other) = a.get(3) {
        let theirs = stats(Path::new(other), &a[1], &a[2]);
        print(other, &theirs);
        let both = ours.textures.keys().filter(|k| theirs.textures.contains_key(*k)).count();
        println!("textures in both: {both}");
        for (label, x, y) in [("only first", &ours, &theirs), ("only second", &theirs, &ours)] {
            let mut only: Vec<(&String, &usize)> = x.textures.iter().filter(|(k, _)| !y.textures.contains_key(*k)).collect();
            let refs: usize = only.iter().map(|(_, n)| **n).sum();
            only.sort_by(|p, q| q.1.cmp(p.1));
            println!("{label}: {} textures, {refs} references", only.len());
            for (k, n) in only.iter().take(15) {
                println!("  {n:>6} {k}");
            }
        }
    }
}
