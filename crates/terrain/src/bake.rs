//! Shared terrain inputs: decoded source textures and decoded LAND records.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use lodforge_archive::Vfs;
use lodforge_esp::{Land, LoadOrder, Worldspace};

use crate::image::Image;
use crate::{Error, Result};

/// Full decoded mip chain of a source texture.
pub struct Texture {
    pub mips: Vec<Image>,
}

/// Decoded source textures, shared across threads; loads on first use.
pub struct TextureCache<'a> {
    vfs: &'a Vfs,
    map: Mutex<HashMap<String, Option<Arc<Texture>>>>,
}

impl<'a> TextureCache<'a> {
    pub fn new(vfs: &'a Vfs) -> Self {
        TextureCache { vfs, map: Mutex::new(HashMap::new()) }
    }

    /// `path` as stored in TXST (relative to Textures\) or Data-relative.
    pub fn get(&self, path: &str) -> Option<Arc<Texture>> {
        let key = texture_path(path);
        if let Some(t) = self.map.lock().unwrap().get(&key) {
            return t.clone();
        }
        let loaded = self.load(&key).map(Arc::new);
        self.map.lock().unwrap().insert(key, loaded.clone());
        loaded
    }

    fn load(&self, key: &str) -> Option<Texture> {
        let bytes = self.vfs.read(key).ok()??;
        let info = lodforge_dds::parse(&bytes).ok()?;
        let mips = (0..info.mip_count)
            .map(|l| lodforge_dds::decode(&bytes, &info, l).map(|i| Image::from_rgba8(i.width, i.height, &i.rgba)))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some(Texture { mips })
    }

    pub fn loaded(&self) -> usize {
        self.map.lock().unwrap().values().filter(|v| v.is_some()).count()
    }
}

/// TXST paths are relative to `Data\Textures\`; normalise to a Data-relative path.
pub fn texture_path(p: &str) -> String {
    let n = lodforge_archive::normalize(p);
    if n.starts_with("textures\\") { n } else { format!("textures\\{n}") }
}

/// Decoded LAND per cell for the given cells (those without LAND are skipped); duplicate LANDs
/// are resolved by [`Worldspace::land_by_cell`].
pub fn decode_lands(lo: &LoadOrder, world: &Worldspace, cells: impl Iterator<Item = (i32, i32)>) -> Result<HashMap<(i32, i32), Land>> {
    let by_cell = world.land_by_cell();
    let wanted: Vec<_> = cells.filter_map(|c| by_cell.get(&c).copied()).collect();
    wanted
        .par_iter()
        .map(|l| {
            let lp = &lo.plugins[l.plugin as usize];
            let rec = lp.record_at(l.offset)?;
            Ok((l.cell, lp.decode_land(&rec)?))
        })
        .collect::<Result<_, lodforge_esp::Error>>()
        .map_err(Error::from)
}
