//! Joins a `LODFORGE_TRACE_COLORS` trace (one line per placed source part, with three sample
//! world positions) with the golden BTOs: for each part, what the golden holds at those
//! positions (shape name, colour array present, and the vertex colour there). Prints the trace
//! line with `gold=<name>:<kind>` per sample appended, where kind is `none` (no colour array),
//! `white` (opaque white), `walpha` (white with alpha < 255), `rgb`, or `-` (position not found).
//! `cargo run -p lodforge-nif --example trace_golden_colors -- <trace.tsv> <golden objects dir> <world>`

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

use lodforge_nif::{Block, parse};

type Map = HashMap<[i64; 3], (String, Option<[u8; 4]>)>;

fn key(p: [f32; 3]) -> [i64; 3] {
    [(p[0] * 2.0).round() as i64, (p[1] * 2.0).round() as i64, (p[2] * 2.0).round() as i64]
}

fn load(path: &std::path::Path) -> Option<Map> {
    let nif = parse(&std::fs::read(path).ok()?).ok()?;
    let mut map = Map::new();
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let name = s.av.net.name.clone().unwrap_or_default();
        let t = &s.av.transform;
        for (i, p) in s.positions.iter().enumerate() {
            let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
            map.entry(key(w)).or_insert((name.clone(), s.colors.get(i).copied()));
        }
    }
    Some(map)
}

fn lookup(map: &Map, p: [f32; 3]) -> String {
    let k = key(p);
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if let Some((name, c)) = map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                    let kind = match c {
                        None => "none".to_string(),
                        Some([255, 255, 255, 255]) => "white".to_string(),
                        Some([255, 255, 255, a]) => format!("walpha{a}"),
                        Some(c) => format!("rgb{},{},{},{}", c[0], c[1], c[2], c[3]),
                    };
                    return format!("{name}:{kind}");
                }
            }
        }
    }
    "-".to_string()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (trace, dir, world) = (&a[0], std::path::Path::new(&a[1]), &a[2]);
    let mut maps: HashMap<String, Option<Map>> = HashMap::new();
    let out = std::io::stdout();
    let mut out = out.lock();
    for line in BufReader::new(std::fs::File::open(trace).unwrap()).lines() {
        let line = line.unwrap();
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 22 || f[0] != "trace-colors" {
            continue;
        }
        let quad = f[1].to_string();
        let map = maps.entry(quad.clone()).or_insert_with(|| load(&dir.join(format!("{world}.{quad}.bto"))));
        let Some(map) = map else {
            writeln!(out, "{line}\tgold=missing").unwrap();
            continue;
        };
        let mut golds = Vec::new();
        for s in &f[19..22] {
            let v = s.trim_start_matches(|c| c != '=').trim_start_matches('=');
            let v = v.split('/').next().unwrap_or("");
            let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            golds.push(if p.len() == 3 { lookup(map, [p[0], p[1], p[2]]) } else { "-".to_string() });
        }
        writeln!(out, "{line}\tgold={}\tgold={}\tgold={}", golds[0], golds[1], golds[2]).unwrap();
    }
}
