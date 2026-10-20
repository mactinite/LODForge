//! Shape statistics over a folder of BTO files: counts by (name, shader flags, clamp mode,
//! vertex colours, large-ref extra data, shader type) and files whose shapes share a grouping key
//! (name + flags + clamp + textures + colours) more than once, to settle the merge key and the HD
//! flag rule empirically. `cargo run -p lodforge-nif --example bto_flags -- <dir> <level> [season]`

use std::collections::BTreeMap;

use lodforge_nif::{Block, parse};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: bto_flags <dir> <level> [season]");
    let level = args.next().expect("level");
    let season = args.next();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut dup_files = 0usize;
    let mut files = 0usize;
    let mut dup_examples: Vec<String> = Vec::new();
    let mut max_verts = 0usize;
    for e in std::fs::read_dir(&dir).expect("dir").flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<&str> = name.split('.').collect();
        if !name.ends_with(".bto") || parts.get(1) != Some(&level.as_str()) {
            continue;
        }
        let has_season = parts.len() == 6;
        match &season {
            Some(s) => {
                if !has_season || parts[4] != s {
                    continue;
                }
            }
            None => {
                if has_season {
                    continue;
                }
            }
        }
        let Ok(nif) = parse(&std::fs::read(e.path()).unwrap()) else { continue };
        files += 1;
        let mut keys: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let n = s.av.net.name.clone().unwrap_or_default();
            let colors = !s.colors.is_empty();
            let large = !s.av.net.extra_data.is_empty();
            let (kind, key) = match nif.get(s.shader_property) {
                Some(Block::LightingShader(l)) => {
                    let t = match nif.get(l.texture_set) {
                        Some(Block::TextureSet(t)) => t.join(",").to_lowercase(),
                        _ => String::new(),
                    };
                    let first = t.split(',').next().unwrap_or("").to_string();
                    let page = if first.contains("objectslod") || first.contains("dyndolod_") {
                        "atlas".to_string()
                    } else if std::env::var_os("BTO_FLAGS_TEXTURES").is_some() {
                        first.rsplit('\\').next().unwrap_or("").to_string()
                    } else {
                        "own".to_string()
                    };
                    (
                        format!("{n:24} type={} f1={:#x} f2={:#x} clamp={} col={} large={} tex={page}", l.shader_type, l.flags1, l.flags2, l.texture_clamp_mode, u8::from(colors), u8::from(large)),
                        format!("{n}|{:#x}|{:#x}|{}|{}|{}", l.flags1, l.flags2, l.texture_clamp_mode, colors, t),
                    )
                }
                Some(Block::EffectShader(x)) => (format!("{n:24} effect f1={:#x} f2={:#x} col={} large={}", x.flags1, x.flags2, u8::from(colors), u8::from(large)), format!("{n}|effect|{}", x.source_texture)),
                _ => (format!("{n:24} no shader"), n.clone()),
            };
            *by_kind.entry(kind).or_default() += 1;
            keys.entry(key).or_default().push(s.positions.len());
            max_verts = max_verts.max(s.positions.len());
        }
        let dups: Vec<(&String, &Vec<usize>)> = keys.iter().filter(|(_, v)| v.len() > 1).collect();
        if !dups.is_empty() {
            dup_files += 1;
            if dup_examples.len() < 8 {
                for (k, v) in dups.iter().take(2) {
                    dup_examples.push(format!("{name}: {v:?} {k}"));
                }
            }
        }
    }
    println!("{files} files, level {level}, max vertices per shape {max_verts}");
    for (k, v) in &by_kind {
        println!("{v:6}  {k}");
    }
    println!("{dup_files} files with a repeated grouping key (name|f1|f2|clamp|colours|textures):");
    for d in dup_examples {
        println!("  {d}");
    }
}
