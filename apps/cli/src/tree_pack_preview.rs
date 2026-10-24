//! `lodforge tree-pack-preview`: CPU render of meshes from a `TreeMeshes.bin` (side view, albedo
//! from the atlas pages through the pack UVs, alpha test at the page threshold) into a PNG, to
//! check positions, UV remap, page assignment and alpha together.

use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_instances::meshpack::MeshPack;

#[derive(clap::Args)]
pub struct Args {
    /// Data folder holding `SKSE\Plugins\LODForgeRuntime\TreeMeshes.bin` and the atlas pages.
    #[arg(long)]
    pub data: PathBuf,
    /// Mesh indices (default: the first 8).
    #[arg(long, value_delimiter = ',')]
    pub meshes: Vec<u32>,
    /// Size of each mesh's square, pixels.
    #[arg(long, default_value_t = 384)]
    pub size: u32,
    #[arg(long)]
    pub out: PathBuf,
    /// Pack file name (`TreeMeshes.bin` or `StaticMeshes.bin`).
    #[arg(long, default_value = "TreeMeshes.bin")]
    pub file: String,
}

pub fn run(a: &Args) -> Result<()> {
    let dir = crate::instances::out_dir(&a.data);
    let pack = MeshPack::from_bytes(&std::fs::read(dir.join(&a.file))?)?;
    let mut pages: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for p in &pack.pages {
        let path = a.data.join(&p.albedo);
        let b = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let info = lodforge_dds::parse(&b)?;
        let img = lodforge_dds::decode(&b, &info, 0)?;
        pages.push((img.width, img.height, img.rgba));
    }
    let ids: Vec<u32> = if a.meshes.is_empty() { (0..pack.meshes.len().min(8) as u32).collect() } else { a.meshes.clone() };
    let s = a.size;
    let (w, h) = (s * ids.len() as u32, s);
    let mut px = vec![40u8; (w * h * 3) as usize];
    for (slot, &mi) in ids.iter().enumerate() {
        let Some(m) = pack.meshes.get(mi as usize) else { continue };
        let b = m.bounds;
        let ext = (b[3] - b[0]).max(b[5] - b[2]).max(b[4] - b[1]).max(1.0);
        // Side view along +y: screen x = model x, screen y = model z (up).
        let to = |p: [f32; 3]| -> (f32, f32, f32) {
            let x = (p[0] - (b[0] + b[3]) * 0.5) / ext * 0.95 + 0.5;
            let y = 1.0 - ((p[2] - b[2]) / ext * 0.95 + 0.025);
            (slot as f32 * s as f32 + x * s as f32, y * s as f32, p[1])
        };
        let mut depth = vec![f32::MAX; (s * s) as usize];
        for c in &pack.clusters[m.first_cluster as usize..(m.first_cluster + m.cluster_count) as usize] {
            let (pw, ph, page) = &pages[usize::from(c.page)];
            let thr = pack.pages[usize::from(c.page)].alpha_threshold;
            for t in 0..usize::from(c.triangles) {
                let vi: Vec<&lodforge_instances::meshpack::Vertex> = (0..3).map(|k| &pack.vertices[pack.indices[c.first_index as usize + 3 * t + k] as usize]).collect();
                let sp: Vec<(f32, f32, f32)> = vi.iter().map(|v| to(v.pos)).collect();
                let (x0, x1) = (sp.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(slot as f32 * s as f32) as i32, sp.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().min(((slot as u32 + 1) * s - 1) as f32) as i32);
                let (y0, y1) = (sp.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as i32, sp.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().min((s - 1) as f32) as i32);
                let area = (sp[1].0 - sp[0].0) * (sp[2].1 - sp[0].1) - (sp[2].0 - sp[0].0) * (sp[1].1 - sp[0].1);
                if area.abs() < 1e-6 {
                    continue;
                }
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let e = |a: (f32, f32, f32), b: (f32, f32, f32)| ((b.0 - a.0) * (fy - a.1) - (b.1 - a.1) * (fx - a.0)) / area;
                        let (l0, l1, l2) = (e(sp[1], sp[2]), e(sp[2], sp[0]), e(sp[0], sp[1]));
                        if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                            continue;
                        }
                        let z = l0 * sp[0].2 + l1 * sp[1].2 + l2 * sp[2].2;
                        let u = (l0 * f32::from(vi[0].uv[0]) + l1 * f32::from(vi[1].uv[0]) + l2 * f32::from(vi[2].uv[0])) / 65535.0;
                        let v = (l0 * f32::from(vi[0].uv[1]) + l1 * f32::from(vi[1].uv[1]) + l2 * f32::from(vi[2].uv[1])) / 65535.0;
                        let (tx, ty) = (((u * *pw as f32) as u32).min(pw - 1), ((v * *ph as f32) as u32).min(ph - 1));
                        let tp = &page[((ty * pw + tx) * 4) as usize..][..4];
                        if tp[3] < thr {
                            continue;
                        }
                        let di = (y as u32 * s + (x as u32 - slot as u32 * s)) as usize;
                        if z >= depth[di] {
                            continue;
                        }
                        depth[di] = z;
                        let o = ((y as u32 * w + x as u32) * 3) as usize;
                        px[o..o + 3].copy_from_slice(&tp[..3]);
                    }
                }
            }
        }
    }
    let f = std::fs::File::create(&a.out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&px)?;
    for &mi in &ids {
        if let Some(m) = pack.meshes.get(mi as usize) {
            println!("  {mi}: {} ({} triangles, {} clusters)", m.model, m.triangles, m.cluster_count);
        }
    }
    println!("wrote {}", a.out.display());
    Ok(())
}
