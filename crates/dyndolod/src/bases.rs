//! Base records DynDOLOD considers for LOD (`BaseSignatures`, default
//! `STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR`): winning override, editor ID, model, record flags, STAT
//! MNAM LOD slots and DNAM directional material.

use std::collections::HashMap;

use lodforge_esp::{LoadOrder, Sig, fields, winning_records};

pub const BASE_SIGNATURES: [&[u8; 4]; 7] = [b"STAT", b"TREE", b"ACTI", b"MSTT", b"CONT", b"FURN", b"DOOR"];

#[derive(Debug, Clone, Default)]
pub struct Base {
    pub signature: [u8; 4],
    pub form_id: u32,
    pub editor_id: String,
    pub flags: u32,
    /// MODL as stored (no `Meshes\` prefix in most plugins).
    pub model: String,
    pub mnam: Option<[String; 4]>,
    /// DNAM material (global form id of a MATO), 0 for none.
    pub material: u32,
    /// Index of the plugin with the winning override.
    pub winning_plugin: u16,
    /// Raw DNAM (STAT: max angle, material, ...).
    pub dnam: Vec<u8>,
    /// Type data a copy of the base keeps: TREE CNAM, MSTT DATA.
    pub type_data: Vec<u8>,
    /// ACTI WNAM water type (global form id), 0 for none. Water activators need it on their
    /// dynamic LOD copy, or the engine draws the water mesh black.
    pub water_type: u32,
    /// OBND (x1, y1, z1, x2, y2, z2).
    pub bounds: [i16; 6],
    /// MODS alternate textures of the model.
    pub alt_textures: Vec<AltTexture>,
}

/// One MODS entry: the shape (3D name and index) whose textures a texture set replaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AltTexture {
    pub shape: String,
    /// Global form id of the TXST.
    pub txst: u32,
    pub index: i32,
}

/// MODS: count, then per entry the 3D name (length-prefixed), the TXST and the 3D index.
pub fn parse_mods(d: &[u8], global: impl Fn(u32) -> Option<u32>) -> Vec<AltTexture> {
    let u32_at = |p: usize| d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut out = Vec::new();
    let Some(n) = u32_at(0) else { return out };
    let mut p = 4;
    for _ in 0..n {
        let Some(len) = u32_at(p) else { break };
        let len = len as usize;
        let Some(name) = d.get(p + 4..p + 4 + len) else { break };
        p += 4 + len;
        let (Some(txst), Some(index)) = (u32_at(p), u32_at(p + 4)) else { break };
        p += 8;
        out.push(AltTexture { shape: zstr(name), txst: global(txst).unwrap_or(0), index: index as i32 });
    }
    out
}

fn zstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

pub fn read_bases(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, Base>> {
    let mut out = HashMap::new();
    for sig in BASE_SIGNATURES {
        for (id, (pi, r)) in winning_records(lo, Sig(*sig))? {
            let lp = &lo.plugins[pi as usize];
            let data = r.data()?;
            let mut b = Base { signature: *sig, form_id: id, flags: r.flags, winning_plugin: pi, ..Default::default() };
            for f in fields(&data) {
                match &f.sig.0 {
                    b"EDID" => b.editor_id = f.zstring(),
                    b"MODL" => b.model = f.zstring(),
                    b"MODS" => b.alt_textures = parse_mods(f.data, |t| lp.global_form_id(t)),
                    b"OBND" if f.data.len() >= 12 => {
                        for (k, v) in b.bounds.iter_mut().enumerate() {
                            *v = i16::from_le_bytes([f.data[k * 2], f.data[k * 2 + 1]]);
                        }
                    }
                    b"DNAM" if sig == b"STAT" && f.data.len() >= 8 => {
                        b.dnam = f.data.to_vec();
                        let m = u32::from_le_bytes([f.data[4], f.data[5], f.data[6], f.data[7]]);
                        b.material = if m == 0 { 0 } else { lp.global_form_id(m).unwrap_or(0) };
                    }
                    b"CNAM" if sig == b"TREE" => b.type_data = f.data.to_vec(),
                    b"DATA" if sig == b"MSTT" => b.type_data = f.data.to_vec(),
                    b"WNAM" if sig == b"ACTI" && f.data.len() >= 4 => {
                        let w = u32::from_le_bytes([f.data[0], f.data[1], f.data[2], f.data[3]]);
                        b.water_type = if w == 0 { 0 } else { lp.global_form_id(w).unwrap_or(0) };
                    }
                    b"MNAM" if sig == b"STAT" => {
                        let mut l: [String; 4] = Default::default();
                        for (k, s) in l.iter_mut().enumerate() {
                            if let Some(d) = f.data.get(k * 260..(k + 1) * 260) {
                                *s = zstr(d);
                            }
                        }
                        b.mnam = Some(l);
                    }
                    _ => {}
                }
            }
            out.insert(id, b);
        }
    }
    Ok(out)
}

/// Key of the plugin that owns a load-order form id: the top byte for full plugins,
/// `0xFE000 | light index` for light plugins.
pub fn owner_key(form_id: u32) -> u32 {
    if form_id >> 24 == 0xFE { form_id >> 12 } else { form_id >> 24 }
}

/// [`owner_key`] -> load-order index.
pub fn owners(lo: &LoadOrder) -> HashMap<u32, usize> {
    lo.plugins.iter().enumerate().map(|(i, p)| (owner_key(p.index.global(0)), i)).collect()
}

/// Load-order form id of `plugin;local` (DynDOLOD config notation), if the plugin is loaded.
pub fn global_id(lo: &LoadOrder, plugin: &str, local: u32) -> Option<u32> {
    lo.find(plugin).map(|p| p.index.global(local))
}

/// `Plugin;FormID[;...]` lines of a config file as load-order form ids.
pub fn read_form_list(lo: &LoadOrder, text: &str) -> std::collections::HashSet<u32> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .filter_map(|l| {
            let mut f = l.split(';');
            let (p, id) = (f.next()?.trim(), f.next()?.trim());
            global_id(lo, p, u32::from_str_radix(id, 16).ok()?)
        })
        .collect()
}

/// Editor IDs of the winning MATO records.
pub fn read_materials(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, String>> {
    let mut out = HashMap::new();
    for (id, (_, r)) in winning_records(lo, Sig(*b"MATO"))? {
        let data = r.data()?;
        out.insert(id, fields(&data).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring()).unwrap_or_default());
    }
    Ok(out)
}

/// Data-relative model path with a `meshes\` prefix (lower-case).
pub fn model_path(model: &str) -> String {
    let m = model.replace('/', "\\").to_ascii_lowercase();
    let m = m.trim_start_matches('\\');
    if let Some(p) = m.find("meshes\\") { m[p..].to_string() } else { format!("meshes\\{m}") }
}
