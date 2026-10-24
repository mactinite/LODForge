//! The GRAS table of a load order: which grass types landscape textures use and whether their
//! models load. `--grass bto` matches NGIO cache files against it and `--grass go` builds its
//! LOD meshes from it; neither places any grass.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use lodforge_esp::{fields, winning_records, Sig};
use lodforge_grass::cgid::{strip_meshes, Gras};

use crate::profile::Profile;

fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Grass list (indices into the GRAS table) of every LTEX that has one.
type LtexGrass = HashMap<u32, Vec<usize>>;

/// Every GRAS (winning record) and every LTEX grass list (GNAM order).
pub fn grass_tables(p: &Profile) -> Result<(Vec<Gras>, LtexGrass)> {
    let lo = &p.load_order;
    let mut gras = Vec::new();
    let mut index = HashMap::new();
    let mut recs: Vec<_> = winning_records(lo, Sig(*b"GRAS"))?.into_iter().collect();
    recs.sort_by_key(|(id, _)| *id);
    for (id, (_, r)) in recs {
        let data = r.data()?;
        let mut g = Gras {
            form_id: id,
            model: String::new(),
            density: 0,
            min_slope: 0,
            max_slope: 0,
            units_from_water: 0,
            water_state: 0,
            position_range: 0.0,
            height_range: 0.0,
            color_range: 0.0,
            wave_period: 0.0,
            flags: 0,
        };
        for f in fields(&data) {
            match &f.sig.0 {
                b"MODL" => g.model = f.zstring(),
                b"DATA" if f.data.len() >= 0x1D => {
                    let d = f.data;
                    g.density = d[0];
                    g.min_slope = d[1];
                    g.max_slope = d[2];
                    g.units_from_water = u16::from_le_bytes([d[4], d[5]]);
                    g.water_state = le_u32(d, 8);
                    g.position_range = le_f32(d, 0x0C);
                    g.height_range = le_f32(d, 0x10);
                    g.color_range = le_f32(d, 0x14);
                    g.wave_period = le_f32(d, 0x18);
                    g.flags = d[0x1C];
                }
                _ => {}
            }
        }
        index.insert(id, gras.len());
        gras.push(g);
    }
    let mut ltex_grass = HashMap::new();
    for (id, (pi, r)) in winning_records(lo, Sig(*b"LTEX"))? {
        let lp = &lo.plugins[pi as usize];
        let data = r.data()?;
        let mut list = Vec::new();
        for f in fields(&data) {
            match &f.sig.0 {
                b"GNAM" if f.data.len() >= 4 => {
                    if let Some(&gi) = lp.global_form_id(le_u32(f.data, 0)).and_then(|g| index.get(&g)) {
                        list.push(gi);
                    }
                }
                _ => {}
            }
        }
        if !list.is_empty() {
            ltex_grass.insert(id, list);
        }
    }
    Ok((gras, ltex_grass))
}

/// maxInstancesPerGroup: as many instances as fit 65535 indices (`65535 / (3 * triangles)`; matches
/// every cap seen in an in-game cache, e.g. 873 for Field_Dry's 25 triangles).
fn group_cap(p: &Profile, model: &str) -> Option<(u32, usize, usize)> {
    let bytes = p.vfs.read(&format!("meshes\\{}", strip_meshes(model))).ok()??;
    let nif = lodforge_nif::parse(&bytes).ok()?;
    let (verts, tris) = nif.blocks.iter().find_map(|b| match b {
        lodforge_nif::Block::TriShape(t) => Some((t.positions.len(), t.triangles.len())),
        _ => None,
    })?;
    if verts == 0 || tris == 0 {
        return None;
    }
    Some(((65535 / (3 * tris)) as u32, verts, tris))
}


/// The grass types of a load order.
pub struct GrassTypes {
    pub gras: Vec<Gras>,
    /// The grass model loads (a type without a loadable model gets no billboard).
    pub model_ok: Vec<bool>,
    /// Grass types referenced by a landscape texture.
    pub used: HashSet<usize>,
}

pub fn grass_types(p: &Profile) -> Result<GrassTypes> {
    let (gras, ltex_grass) = grass_tables(p)?;
    let used: HashSet<usize> = ltex_grass.values().flatten().copied().collect();
    let mut model_ok = vec![false; gras.len()];
    let mut cap_by_model: HashMap<String, Option<(u32, usize, usize)>> = HashMap::new();
    for &i in &used {
        let key = strip_meshes(&gras[i].model).to_ascii_lowercase();
        let c = *cap_by_model.entry(key).or_insert_with(|| group_cap(p, &gras[i].model));
        model_ok[i] = c.is_some();
    }
    Ok(GrassTypes { gras, model_ok, used })
}
