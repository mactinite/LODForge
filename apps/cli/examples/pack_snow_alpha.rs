//! Vertex alpha of the static pack's snow draw pages (the baked snow mask) against the other pages.
//! `pack_snow_alpha <StaticMeshes.bin>`
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let pack = lodforge_instances::meshpack::MeshPack::from_bytes(&std::fs::read(path).unwrap()).unwrap();
    let snow_pages = pack.pages.iter().filter(|p| p.snow.is_some()).count();
    println!("{} pages, {} with a snow material", pack.pages.len(), snow_pages);
    let mut hist = [[0u64; 8]; 2];
    for c in &pack.clusters {
        let s = usize::from(pack.pages[usize::from(c.page)].snow.is_some());
        for k in 0..usize::from(c.triangles) * 3 {
            let v = pack.indices[c.first_index as usize + k] as usize;
            hist[s][usize::from(pack.vertices[v].color[3]) / 32] += 1;
        }
    }
    for (s, h) in hist.iter().enumerate() {
        let t: u64 = h.iter().sum::<u64>().max(1);
        println!("{} pages: alpha /32 buckets {:?}", if s == 1 { "snow" } else { "other" }, h.iter().map(|n| format!("{:.1}%", 100.0 * *n as f64 / t as f64)).collect::<Vec<_>>());
    }
    let black = pack.pages.iter().filter(|p| p.snow.is_some_and(|s| s.color == [0.0; 3])).count();
    let keys: std::collections::HashSet<String> = pack.pages.iter().filter_map(|p| p.snow.map(|s| s.key())).collect();
    println!("{black} snow pages with colour 0 (multi-pass), {} distinct snow materials", keys.len());
    for p in pack.pages.iter().filter(|p| p.snow.is_some()).take(5) {
        println!("  {} class {} {:?}", p.albedo, p.class, p.snow.unwrap());
    }
}
