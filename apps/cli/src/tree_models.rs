//! `lodforge tree-models`: generated 3D tree LOD models (Ultra phase 3,
//! docs/ultra-tree-models-plan.md). So far `--inspect <model>` prints how a tree splits into bark
//! and foliage parts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use lodforge_nif::Block;
use lodforge_treemodel::{AlphaImage, Kind, collect, split};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Print the bark/foliage split of these full models (paths below `meshes\`, comma separated).
    #[arg(long, value_delimiter = ',')]
    pub inspect: Vec<String>,
}

/// Texture alpha through the VFS, decoded once per path.
pub struct Textures<'a> {
    p: &'a Profile,
    cache: Mutex<HashMap<String, Option<Arc<AlphaImage>>>>,
}

impl<'a> Textures<'a> {
    pub fn new(p: &'a Profile) -> Self {
        Textures { p, cache: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, path: &str) -> Option<Arc<AlphaImage>> {
        if let Some(v) = self.cache.lock().unwrap().get(path) {
            return v.clone();
        }
        let img = (|| {
            let bytes = self.p.vfs.read(path).ok()??;
            let info = lodforge_dds::parse(&bytes).ok()?;
            let img = lodforge_dds::decode(&bytes, &info, 0).ok()?;
            Some(Arc::new(AlphaImage::from_rgba(img.width, img.height, &img.rgba)))
        })();
        self.cache.lock().unwrap().insert(path.to_string(), img.clone());
        img
    }
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let textures = Textures::new(&p);
    for m in &a.inspect {
        let path = lodforge_dyndolod::bases::model_path(m);
        let bytes = p.vfs.read(&path)?.with_context(|| format!("{path} not found"))?;
        let nif = lodforge_nif::parse_all_geometry(&bytes).with_context(|| format!("parsing {path}"))?;
        let shapes = collect(&nif);
        let mut notes = Default::default();
        let parts = split(&nif, &shapes, &|t| textures.get(t), &mut notes);
        println!("{path}: {} source shapes", shapes.len());
        for (i, s) in shapes.iter().enumerate() {
            let Some(Block::TriShape(ts)) = nif.blocks.get(s.block) else { continue };
            let diffuse = lodforge_treemodel::split::diffuse(&nif, ts).unwrap_or_default();
            let xf = if s.xf.is_identity() { String::new() } else { format!(" scale {:.3} offset {:?}", s.xf.s, s.xf.t) };
            println!("  #{i} {:?} [{}] {} tris{}{} {diffuse}", s.name, nif.block_type(s.block), s.triangles, if s.skinned { " skinned" } else { "" }, xf);
            for pt in parts.iter().filter(|pt| pt.shape == i) {
                println!("      -> {:?} {} tris ({:?}){}", pt.kind, pt.triangles, pt.reason, pt.vertices.as_ref().map(|v| format!(", {} of {} vertices", v.len(), ts.positions.len())).unwrap_or_default());
            }
        }
        let sum = |k: Kind| parts.iter().filter(|pt| pt.kind == k).map(|pt| pt.triangles).sum::<usize>();
        println!("  crown {} tris, trunk {} tris{}", sum(Kind::Crown), sum(Kind::Trunk), if notes.0.is_empty() { String::new() } else { format!("; notes: {}", notes.0.join(", ")) });
    }
    Ok(())
}
