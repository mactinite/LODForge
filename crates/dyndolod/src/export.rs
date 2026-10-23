//! DynDOLOD's LODGen export (`Edit Scripts\Export\LODGen_SSE_Export_<World>.txt`): header keys
//! and one 17-column reference line per object LOD reference (docs/re/dyndolod-spec.md "The
//! LODGen export file").

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use lodforge_esp::Worldspace;
use lodforge_esp::record::{FLAG_DELETED, FLAG_INITIALLY_DISABLED};

use crate::assign::{BaseLod, Resolver};
use crate::bases::Base;
use crate::rules::{Rule, select_reference_rule};

/// One reference line.
#[derive(Debug, Clone, Default)]
pub struct RefLine {
    pub ref_id: u32,
    pub ref_flags: u32,
    pub position: [f32; 3],
    /// Degrees.
    pub rotation: [f64; 3],
    pub scale: f32,
    pub editor_id: String,
    pub base_flags: u32,
    pub material: String,
    pub full_model: String,
    pub columns: [String; 4],
    /// Base record (after season swaps); not part of the line.
    pub base: u32,
}

/// Delphi `Format('%.6f')`: the exact binary value rounded half away from zero; no sign on zero.
///
/// Delphi's Win64 float-to-text first scales the value to an 18-digit integer in double
/// precision (`|v| * 10^(17 - floor(log10 |v|))`), so exact binary ties at the 7th decimal round
/// by that product's rounding, not by the true value. This reproduces 99.75% of the golden
/// export's positions; values in roughly 10000..16000 sometimes scale by one more digit.
pub fn fmt6(v: f64) -> String {
    let a = v.abs();
    let s = if a == 0.0 || !a.is_finite() {
        format!("{a:.40}")
    } else {
        let k = 17 - a.log10().floor() as i32;
        let p: f64 = format!("1e{k}").parse().unwrap_or(1.0);
        let n = (a * p).round();
        // n has 18 digits; put the decimal point k digits from the right.
        let digits = format!("{:0>width$}", n as u128, width = (k.max(0) as usize) + 1);
        if k > 0 {
            let (i, f) = digits.split_at(digits.len() - k as usize);
            format!("{i}.{f}")
        } else {
            format!("{digits}.0")
        }
    };
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes().take(6)).collect();
    if frac.as_bytes().get(6).is_some_and(|&d| d >= b'5') {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let n = digits.len() - 6;
    let text = format!("{}.{}", String::from_utf8_lossy(&digits[..n]), String::from_utf8_lossy(&digits[n..]));
    if v < 0.0 && digits.iter().any(|&d| d != b'0') { format!("-{text}") } else { text }
}

/// The full model column: `Meshes\` + the lower-case path below it.
pub fn export_model_text(model: &str) -> String {
    let m = crate::bases::model_path(model);
    format!("Meshes\\{}", m.strip_prefix("meshes\\").unwrap_or(&m))
}

impl RefLine {
    pub fn text(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "{:08X}\t{:08X}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:08X}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.ref_id,
            self.ref_flags,
            pos_text(self.position[0]),
            pos_text(self.position[1]),
            pos_text(self.position[2]),
            fmt6(self.rotation[0]),
            fmt6(self.rotation[1]),
            fmt6(self.rotation[2]),
            fmt6(f64::from(self.scale)),
            self.editor_id,
            self.base_flags,
            self.material,
            self.full_model,
            self.columns[0],
            self.columns[1],
            self.columns[2],
            self.columns[3]
        );
        s
    }
}

/// Why a reference is not in the export (for diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Skip {
    NoBase,
    Disabled,
    EnableParent,
    ReferenceRule,
    NoRule,
    NoColumns,
    IgnoredEdid,
    TreeLod,
    /// `mod_world_ignore.txt` lists the defining or winning plugin for this worldspace.
    ModWorldIgnore,
}

pub struct ExportInput<'a> {
    pub world: &'a Worldspace,
    pub bases: &'a HashMap<u32, Base>,
    pub materials: &'a HashMap<u32, String>,
    pub resolver: &'a Resolver<'a>,
    pub rules: &'a [Rule],
    /// Load-order plugin file names.
    pub plugins: &'a [String],
    pub owners: &'a HashMap<u32, usize>,
    pub ignore_base_edid: &'a [String],
    pub large_refs: &'a HashSet<u32>,
    /// Season base swaps (original base -> seasonal base); empty for the default season.
    pub swaps: &'a HashMap<u32, u32>,
    /// Enable parents treated as absent (`xesplod.txt`).
    pub xesp_lod: &'a HashSet<u32>,
    /// `TreeLODMaxTilt` (degrees).
    pub tree_max_tilt: f64,
    /// Child worldspace `Bounds` (SW x, SW y, NE x, NE y cells) where trees use object LOD.
    pub tree_object_bounds: &'a [(i32, i32, i32, i32)],
    /// `Windows`: glow LOD windows (`<model>glow_lod_N.nif`) in object LOD.
    pub windows: bool,
    /// Plugins whose references `mod_world_ignore.txt` ignores in this worldspace (lower-case,
    /// [`crate::childworld::ModWorldIgnore::plugins_for`]).
    pub ignore_plugins: &'a HashSet<String>,
    /// Ultra tree LOD: trees stay in object LOD instead of the game's tree LOD, and a tree level
    /// whose rule wants a LOD model that is not installed uses the billboard
    /// (`TreeFullFallBack=1`).
    pub ultra: bool,
    /// Winning REFR/ACHR states across the load order, including interior enable markers.
    pub reference_states: &'a HashMap<u32, ReferenceState>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReferenceState {
    pub flags: u32,
    pub parent: u32,
    pub opposite: bool,
}

/// Enable parents can live in interior cells or other worldspaces, not just the export world.
pub fn read_reference_states(lo: &lodforge_esp::LoadOrder) -> lodforge_esp::Result<HashMap<u32, ReferenceState>> {
    fn walk(entries: lodforge_esp::Entries<'_>, lp: &lodforge_esp::LoadedPlugin, out: &mut HashMap<u32, ReferenceState>) -> lodforge_esp::Result<()> {
        for entry in entries {
            match entry? {
                lodforge_esp::Entry::Group(g) => walk(g.entries(), lp, out)?,
                lodforge_esp::Entry::Record(r) if matches!(&r.sig.0, b"REFR" | b"ACHR") => {
                    let Some(id) = lp.global_form_id(r.form_id) else { continue };
                    if r.is_deleted() {
                        out.remove(&id);
                        continue;
                    }
                    let mut state = ReferenceState { flags: r.flags, ..Default::default() };
                    for f in lodforge_esp::fields(&r.data()?) {
                        if &f.sig.0 == b"XESP" {
                            if f.data.len() < 8 {
                                return Err(lodforge_esp::Error::MalformedIn { path: lp.plugin.path.clone(), offset: r.offset, what: "truncated XESP" });
                            }
                            let local = u32::from_le_bytes(f.data[..4].try_into().unwrap());
                            state.parent = lp.global_form_id(local).ok_or_else(|| lodforge_esp::Error::MalformedIn { path: lp.plugin.path.clone(), offset: r.offset, what: "unresolved XESP parent" })?;
                            state.opposite = f.data[4] & 1 != 0;
                        }
                    }
                    out.insert(id, state);
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut states = HashMap::new();
    for lp in &lo.plugins {
        walk(lp.plugin.top_entries(), lp, &mut states)?;
    }
    Ok(states)
}

fn parent_state(states: &HashMap<u32, ReferenceState>, mut id: u32, mut link_opposite: bool) -> Result<(u32, bool, bool), String> {
    let mut enabled = true;
    let mut opposite = false;
    let mut seen = HashSet::new();
    loop {
        opposite ^= link_opposite;
        if id == PLAYER_REF {
            return Ok((id, opposite, enabled && !link_opposite));
        }
        if !seen.insert(id) {
            return Err(format!("cycle in enable-parent chain at {id:08X}"));
        }
        let state = states.get(&id).ok_or_else(|| format!("missing enable parent {id:08X}"))?;
        // Initial enablement requires every link's condition, while runtime tracking
        // flattens the chain to the top parent with XOR-combined inversions.
        enabled &= (state.flags & FLAG_INITIALLY_DISABLED == 0) != link_opposite;
        if state.parent == 0 {
            return Ok((id, opposite, enabled));
        }
        link_opposite = state.opposite;
        id = state.parent;
    }
}

/// xEdit's `RoundToEx(v, -digits)` with the factor in single precision and banker's rounding.
pub fn round_to(v: f64, digits: i32) -> f64 {
    let f = f64::from(10f32.powi(-digits));
    (v / f).round_ties_even() * f
}

/// Radians to the degrees DynDOLOD prints: degrees rounded to 4 decimals, stored as single
/// (signed, not normalised).
pub fn degrees(rad: f32) -> f64 {
    let mode = std::env::var("LODFORGE_DEGMODE").unwrap_or_default();
    let d = match mode.as_str() {
        "1" => f64::from(rad * (180.0f32 / std::f32::consts::PI)),
        "2" => f64::from(rad) * f64::from(180.0f32 / std::f32::consts::PI),
        "3" => f64::from(rad) * (180.0 / std::f64::consts::PI),
        _ => f64::from(rad) * 180.0 / std::f64::consts::PI,
    };
    let r = match std::env::var("LODFORGE_RNDMODE").unwrap_or_default().as_str() {
        "1" => (d * 1e4 + 0.5f64.copysign(d)).trunc() / 1e4,
        "2" => format!("{d:.4}").parse::<f64>().unwrap(),
        _ => (d * 1e4).round_ties_even() / 1e4,
    };
    f64::from(r as f32)
}

/// Trees DynDOLOD leaves out of tree LOD for their tilt: xEdit's `max < angle < 360 - max` on the
/// x or y rotation in [`degrees`] (signed, so stored negative angles stay in tree LOD). Same rule
/// as `lodforge_treelod::dyndolod_tilted`.
pub fn tree_tilted(rotation: [f32; 3], max: f64) -> bool {
    let w = |rad: f32| {
        let d = degrees(rad);
        d > max && d < 360.0 - max
    };
    w(rotation[0]) || w(rotation[1])
}

pub fn build_lines(inp: &ExportInput<'_>) -> (Vec<RefLine>, HashMap<u32, Skip>) {
    let mut cache: HashMap<u32, BaseLod> = HashMap::new();
    let mut ids: Vec<u32> = inp.world.refs.keys().copied().collect();
    ids.sort_unstable();
    let mut lines = Vec::new();
    let mut skipped = HashMap::new();
    for id in ids {
        let p = &inp.world.refs[&id];
        let base_id = inp.swaps.get(&p.base).copied().unwrap_or(p.base);
        let Some(b) = inp.bases.get(&base_id) else {
            skipped.insert(id, Skip::NoBase);
            continue;
        };
        if p.flags & (FLAG_DELETED | FLAG_INITIALLY_DISABLED) != 0 {
            skipped.insert(id, Skip::Disabled);
            continue;
        }
        if p.enable_parent && !inp.xesp_lod.contains(&p.enable_parent_id) {
            skipped.insert(id, Skip::EnableParent);
            continue;
        }
        let edid_lc = b.editor_id.to_ascii_lowercase();
        if inp.ignore_base_edid.iter().any(|e| edid_lc.contains(e.as_str())) {
            skipped.insert(id, Skip::IgnoredEdid);
            continue;
        }
        let owner = inp.owners.get(&crate::bases::owner_key(id)).and_then(|&i| inp.plugins.get(i)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let winner = inp.plugins.get(usize::from(p.plugin)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if crate::childworld::mod_world_ignored(inp.ignore_plugins, &owner, &winner) {
            skipped.insert(id, Skip::ModWorldIgnore);
            continue;
        }
        let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
        let lod = cache.entry(base_id).or_insert_with(|| inp.resolver.resolve(b));
        let columns = if let Some(r) = select_reference_rule(inp.rules, &owner, local) {
            // Reference rules: only "Unchanged" ones keep the reference in object LOD (with the
            // rule's own level columns, possibly none).
            if !r.reference_action.eq_ignore_ascii_case("Unchanged") {
                skipped.insert(id, Skip::ReferenceRule);
                continue;
            }
            (inp.resolver.columns(r, b, lod), r.flags, r)
        } else {
            let Some(ri) = lod.rule else {
                skipped.insert(id, Skip::NoRule);
                continue;
            };
            if lod.columns.iter().all(String::is_empty) {
                skipped.insert(id, Skip::NoColumns);
                continue;
            }
            // Trees with billboard rules go to classic tree LOD, except tilted ones and those in
            // child worldspace bounds (tree LOD does not work there).
            if !inp.ultra && (crate::assign::is_tree(b) && lod.billboard.is_some() && std::env::var_os("LODFORGE_TREE_OLD").is_none() || crate::assign::is_tree(b) && inp.rules[ri].levels.iter().any(|l| matches!(l, crate::rules::Level::Billboard(_)))) {
                let cell = ((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32);
                let in_bounds = inp.tree_object_bounds.iter().any(|&(x0, y0, x1, y1)| (x0..=x1).contains(&cell.0) && (y0..=y1).contains(&cell.1));
                // Large references stay in object LOD (golden Solstheim: 94 Dragonborn.esm trees).
                // So do child worldspace copies (golden: copied trees are billboards in object LOD).
                if !tree_tilted(p.rotation, inp.tree_max_tilt) && !in_bounds && !inp.large_refs.contains(&id) && !crate::childworld::is_copy(id) {
                    skipped.insert(id, Skip::TreeLod);
                    continue;
                }
            }
            let mut cols = lod.columns.clone();
            if inp.ultra && crate::assign::is_tree(b) && let Some(bb) = &lod.billboard {
                for (col, level) in cols.iter_mut().zip(inp.rules[ri].levels.iter()) {
                    if col.is_empty() && matches!(level, crate::rules::Level::Model(_)) {
                        col.clone_from(bb);
                    }
                }
            }
            (cols, inp.rules[ri].flags, &inp.rules[ri])
        };
        let (columns, rule_flags, rule) = columns;
        let mut material = String::new();
        let mat = inp.materials.get(&b.material).map(|m| m.to_ascii_lowercase()).unwrap_or_default();
        // NoMATO rules drop the snow/ash material object.
        if mat.contains("snow") && rule_flags & crate::rules::FLAG_NO_MATO == 0 {
            material.push_str("Snow");
        }
        if inp.large_refs.contains(&id) {
            material.push_str("-LargeRef");
        }
        let line = RefLine {
            ref_id: id,
            ref_flags: p.flags,
            position: p.position,
            rotation: [degrees(p.rotation[0]), degrees(p.rotation[1]), degrees(p.rotation[2])],
            scale: if p.has_scale { p.scale } else { 1.0 },
            editor_id: b.editor_id.clone(),
            // LODGen reads 0x40 in this column as "tree".
            base_flags: if &b.signature == b"TREE" { b.flags | 0x40 } else { b.flags },
            material,
            // A child worldspace copy places a base copy showing its LOD4 model.
            full_model: match columns.iter().find(|c| !c.is_empty()) {
                Some(c) if crate::childworld::is_copy(id) && !crate::assign::is_tree(b) => c.clone(),
                _ => export_model_text(&b.model),
            },
            columns,
            base: base_id,
        };
        // Glow windows: a second line for the same reference, before the normal one, with the
        // glow overlay models (NoGlow rules have none).
        if inp.windows && rule_flags & crate::rules::FLAG_NO_GLOW == 0
            && let Some(glow) = inp.resolver.glow_columns(rule, b, &line.columns)
        {
            lines.push(RefLine { columns: glow, ..line.clone() });
        }
        lines.push(line);
    }
    (lines, skipped)
}

/// Original bases whose emitted large-reference object LOD requires HasDistantLOD.
/// Generated child copies use separate bases; dynamic-only references are not sufficient.
pub fn large_object_bases(lines: &[RefLine], large_refs: &HashSet<u32>) -> HashSet<u32> {
    lines.iter()
        .filter(|l| large_refs.contains(&l.ref_id) && !crate::childworld::is_copy(l.ref_id) && l.columns.iter().any(|c| !c.is_empty()))
        .map(|l| l.base)
        .collect()
}

/// Position text (experiment switch LODFORGE_POSMODE: 0 exact half-away, 1 round_to).
pub fn pos_text(v: f32) -> String {
    match std::env::var("LODFORGE_POSMODE").ok().as_deref() {
        Some("1") => fmt6(round_to(f64::from(v), 6)),
        _ => fmt6(f64::from(v)),
    }
}

/// Inputs of the export header (`LODGen_SSE_Export_<World>[_<Season>].txt`).
#[derive(Debug, Clone, Default)]
pub struct Header {
    pub world: String,
    pub south_west: (i32, i32),
    /// DynDOLOD's `Edit Scripts\Export\` folder (with trailing backslash).
    pub export_dir: String,
    /// `_<SEASON>` or empty.
    pub season: String,
    /// LODCoords4/8/16/32 for season exports (quads that differ from the default season).
    pub lod_coords: Option<[Vec<(i32, i32)>; 4]>,
    pub limit_coords4: Vec<(i32, i32)>,
    pub limit_coords8: Vec<(i32, i32)>,
    /// Full paths of the loaded archives, in load order.
    pub resources: Vec<String>,
    /// WRLD TNAM diffuse (`Landscape\...\x.dds`, as stored), if any.
    pub hd_lod_texture: String,
    /// `IgnoreTexture` entries.
    pub ignore_textures: Vec<String>,
    pub remove_unseen_z_shift: f64,
    pub flat_lod_level_flag: i32,
    /// Crown, Trunk, FlatTrunk brightness (R, G, B).
    pub brightness: [[f64; 3]; 3],
    pub path_data: String,
    pub path_output: String,
}

fn coords(list: &[(i32, i32)]) -> String {
    list.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(";")
}

/// Delphi `FloatToStr` (15 significant digits, shortest form).
pub fn float_to_str(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    // As in fmt6: the value is first scaled to an 18-digit integer in double precision, then
    // rounded half up to 15 significant digits.
    let a = v.abs();
    let mut e = a.log10().floor() as i32;
    let p: f64 = format!("1e{}", 17 - e).parse().unwrap_or(1.0);
    let n18 = (a * p).round() as u128;
    let mut n15 = n18 / 1000 + u128::from(n18 % 1000 >= 500);
    if n15 >= 1_000_000_000_000_000 {
        n15 /= 10;
        e += 1;
    }
    let s = format!("{}{}.{}e{e}", if v < 0.0 { "-" } else { "" }, n15 / 100_000_000_000_000, &format!("{n15:015}")[1..]);
    let (m, _) = s.split_once('e').unwrap();
    let neg = m.starts_with('-');
    let digits: String = m.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if e >= 0 && (e as usize) < 15 {
        let ip = e as usize + 1;
        if digits.len() <= ip {
            out.push_str(digits);
            out.push_str(&"0".repeat(ip - digits.len()));
        } else {
            out.push_str(&digits[..ip]);
            out.push('.');
            out.push_str(&digits[ip..]);
        }
    } else if e < 0 && e >= -4 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-e - 1) as usize));
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!("E{}{}", if e < 0 { "-" } else { "+" }, e.abs()));
    }
    out
}

impl Header {
    pub fn text(&self) -> String {
        let mut s = String::new();
        let mut line = |l: String| {
            s.push_str(&l);
            s.push_str("\r\n");
        };
        let w = &self.world;
        let x = &self.export_dir;
        line("GameMode=SSE".into());
        line(format!("Worldspace={w}"));
        line(format!("CellSW={} {}", self.south_west.0, self.south_west.1));
        if let Some(lc) = &self.lod_coords {
            for (i, l) in [(3usize, 32), (2, 16), (1, 8), (0, 4)] {
                line(format!("LODCoords{l}={}", coords(&lc[i])));
            }
        }
        line(format!("TerrainData={x}LODGen_SSE_LODGen_Terrain_{w}.bin"));
        if !self.limit_coords4.is_empty() {
            line(format!("LimitCoords4={}", coords(&self.limit_coords4)));
        }
        if !self.limit_coords8.is_empty() {
            line(format!("LimitCoords8={}", coords(&self.limit_coords8)));
        }
        line("RemoveUnseenFaces=True".into());
        line("DontMergeShapes=False".into());
        line(format!("TextureAtlasMap={x}LODGen_SSE_ObjectAtlasMap_{w}.txt"));
        line(format!("AltTextures={x}LODGen_SSE_AltTextures_{w}{}.txt", self.season));
        line(format!("FlatTextures={x}LODGen_SSE_FlatTextures.txt"));
        line(format!("VC={x}LODGen_SSE_VC_{w}{}.txt", self.season));
        line(format!("CreateHeightMap={}", if self.season.is_empty() { "True" } else { "False" }));
        for r in &self.resources {
            line(format!("Resource={r}"));
        }
        for l in ["Threads=0", "ThreadSplit=0", "UseDecalFlag=False", "Level32=False", "UseOptimizer=false", "UseHDFlag=true", "Experimental=True", "NotHDMeshMask=nif", "IsHDMeshMask=_lod_0", "NotHDTextureMask=lod", "NotHDTextureMask=lowres", "NotHDTextureMask=architecture"] {
            line(l.into());
        }
        // DynDOLOD writes the vanilla HD LOD texture pair for SSE, whatever the WRLD TNAM says.
        line("IsHDTextureMask=Textures\\Landscape\\Mountains\\MountainSlab02.dds".into());
        line("IsHDTextureMask=Textures\\Landscape\\Mountains\\MountainSlab02_N.dds".into());
        for t in &self.ignore_textures {
            line(format!("IsHDTextureMask={t}"));
        }
        line("PassThruMeshMask=glow_lod".into());
        line("PassThruMeshMask=passthru_lod".into());
        line("AtlasTolerance=0.2".into());
        line(format!("RemoveUnseenZShift={}", float_to_str(self.remove_unseen_z_shift)));
        line("MergeVertexColors=true".into());
        line(format!("FlatLODLevelLODFlag={}", self.flat_lod_level_flag));
        for (n, b) in ["Crown", "Trunk", "FlatTrunk"].iter().zip(&self.brightness) {
            for (c, v) in ["R", "G", "B"].iter().zip(b) {
                line(format!("{n}Brightness{c}={}", float_to_str(*v)));
            }
        }
        line(format!("PathData={}", self.path_data));
        line(format!("PathOutput={}", self.path_output));
        if let Some(sea) = self.season.strip_prefix('_') {
            line(format!("Season={sea}"));
        }
        s
    }
}

/// One `LODGen_SSE_FlatTextures.txt` line for a billboard: texture, width, height, shiftZ,
/// scale, lighting effect, flat-normals file per level, glow texture, vertex colour, colour
/// variance, depth, shiftX, shiftY, height variance, back texture. Sizes come from the TexGen
/// `.txt` next to the texture.
pub fn flat_texture_line(texture: &str, txt: &str, normals_file: &str, lighting_effect: f32, vertex_color: f32, color_variance: f32, height_variance: f32) -> String {
    let mut v = std::collections::HashMap::new();
    for l in txt.lines() {
        if let Some((k, val)) = l.split_once('=') {
            v.insert(k.trim().to_ascii_uppercase(), val.trim().parse::<f32>().unwrap_or(0.0));
        }
    }
    let g = |k: &str, d: f32| f64::from(v.get(k).copied().unwrap_or(d));
    let width = g("WIDTH", 0.0);
    let f = |x: f64| float_to_str(x);
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t",
        texture,
        f(width),
        f(g("HEIGHT", 0.0)),
        f(g("SHIFTZ", 0.0)),
        f(g("SCALE", 1.0)),
        f(f64::from(lighting_effect)),
        [normals_file; 4].join(";"),
        "textures\\white.dds",
        f(f64::from(vertex_color)),
        f(f64::from(color_variance)),
        f(v.get("DEPTH").map(|d| f64::from(*d)).unwrap_or(width)),
        f(g("SHIFTX", 0.0)),
        f(g("SHIFTY", 0.0)),
        f(f64::from(height_variance)),
    )
}

/// `<model>_<CRC32>_dyndolod_lod.nif` (CRC32 of the installed full model, the original one for
/// a PGPatcher-patched mesh) or `<model>_dyndolod_lod.nif` from LOD model discovery.
fn dynamic_lod_model(inp: &ExportInput<'_>, b: &Base) -> Option<String> {
    let model = crate::bases::model_path(&b.model);
    let stem = model.rsplit('\\').next().unwrap_or(&model).trim_end_matches(".nif").to_ascii_lowercase();
    let r = inp.resolver;
    if let Some(crc) = r.model_crc(&model)
        && let Some(p) = r.index.get(&format!("{stem}_{crc:08x}_dyndolod_lod.nif"))
    {
        return Some(p.to_string());
    }
    r.index.get(&format!("{stem}_dyndolod_lod.nif")).map(str::to_string)
}

/// The player reference (Skyrim.esm), as an enable parent.
const PLAYER_REF: u32 = 0x14;

/// A reference that gets dynamic LOD (a full-LOD copy the DLL enables beyond the loaded grid).
#[derive(Debug, Clone)]
pub struct DynamicRef {
    pub ref_id: u32,
    pub base: u32,
    pub grid: crate::rules::Grid,
    pub initially_enabled: bool,
    pub enable_parent: u32,
    /// Opposite-of-parent XESP flag.
    pub opposite_parent: bool,
    pub large: bool,
    pub persistent: bool,
    /// Replace the original with a hidden marker; this full-model copy also serves the loaded grid.
    pub replace: bool,
    /// Model for the copy: the dynamic LOD model / LOD level 0, or the full model.
    pub model: String,
}

/// References that get dynamic LOD: their rule (reference or mask) has a grid, and either the
/// rule gives no object LOD levels, the rule has the Dynamic flag, or the reference cannot be in
/// static object LOD (initially disabled or with an enable parent).
pub fn build_dynamic(inp: &ExportInput<'_>) -> Result<Vec<DynamicRef>, String> {
    use crate::rules::{FLAG_DYNAMIC, Grid};
    let full_lod_rule = crate::rules::Rule {
        mask: String::new(), mask_lc: String::new(), reference: None,
        levels: [crate::rules::Level::None; 4], grid: Grid::FarFull,
        reference_action: "Unchanged".into(), flags: 0, source: "Is Full LOD".into(),
    };
    let mut cache: HashMap<u32, BaseLod> = HashMap::new();
    let mut ids: Vec<u32> = inp.world.refs.keys().copied().collect();
    ids.sort_unstable();
    let mut out = Vec::new();
    for id in ids {
        let p = &inp.world.refs[&id];
        let base_id = inp.swaps.get(&p.base).copied().unwrap_or(p.base);
        let Some(b) = inp.bases.get(&base_id) else { continue };
        // Bases without a model (engine markers such as CollisionMarker 00000021, used by
        // collision-box references) have nothing to show at distance. DynDOLOD skips them too.
        if b.model.trim().is_empty() {
            continue;
        }
        // Child worldspace copies are static object LOD only.
        if p.flags & FLAG_DELETED != 0 || crate::childworld::is_copy(id) {
            continue;
        }
        let edid_lc = b.editor_id.to_ascii_lowercase();
        if inp.ignore_base_edid.iter().any(|e| edid_lc.contains(e.as_str())) {
            continue;
        }
        let owner = inp.owners.get(&crate::bases::owner_key(id)).and_then(|&i| inp.plugins.get(i)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let winner = inp.plugins.get(usize::from(p.plugin)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if crate::childworld::mod_world_ignored(inp.ignore_plugins, &owner, &winner) {
            continue;
        }
        let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
        let lod = cache.entry(base_id).or_insert_with(|| inp.resolver.resolve(b));
        let rule = match select_reference_rule(inp.rules, &owner, local) {
            Some(r) => Some(r),
            None => lod.rule.map(|ri| &inp.rules[ri]),
        };
        // Persistent Is Full LOD placements already render at distance without mesh rules.
        // Retain that geometry when the plugin writer removes Is Full LOD and lists a reset.
        let rule = rule.or_else(|| (p.is_persistent() && p.flags & 0x10000 != 0).then_some(&full_lod_rule));
        let Some(r) = rule else { continue };
        let replace = r.reference_action.eq_ignore_ascii_case("Replace");
        if !replace && (r.grid == Grid::None || !r.reference_action.eq_ignore_ascii_case("Unchanged")) {
            continue;
        }
        // Enable parent = the player: opposite means never enabled (how road and tree mods switch
        // off vanilla references they replace; none of ~1,900 such references in the E2E golden),
        // otherwise always enabled (no parent).
        let player_parent = p.enable_parent && p.enable_parent_id == PLAYER_REF;
        if player_parent && p.enable_parent_flags & 1 != 0 {
            continue;
        }
        // Large references already have LOD inside the large-reference grid: DynDOLOD skips
        // them on NearGrid ("Skipping large reference in NearGrid"; `UpgradeLargeReferences=0`).
        if !replace && inp.large_refs.contains(&id) && matches!(r.grid, Grid::NearLod | Grid::NearFull) {
            continue;
        }
        let has_parent = p.enable_parent && !player_parent && !inp.xesp_lod.contains(&p.enable_parent_id);
        let disabled = p.flags & FLAG_INITIALLY_DISABLED != 0;
        if !replace && r.has_levels() && r.flags & FLAG_DYNAMIC == 0 && !has_parent && !disabled {
            continue;
        }
        // `… Full` grids show the full model. `… LOD` grids show the dedicated dynamic LOD model
        // (`<model>[_<CRC32>]_dyndolod_lod.nif`), else the full model when the rule's first
        // column says so, else the level-0 LOD model. Unchanged references without a dedicated
        // model retain their full model (doors, rapids and enable-parent trees).
        let full = replace || matches!(r.grid, Grid::NearFull | Grid::FarFull | Grid::NeverFadeFull);
        let model = if full {
            let path = crate::bases::model_path(&b.model);
            if !(inp.resolver.exists)(&path) {
                return Err(format!("dynamic reference {id:08X}: missing full model {path}"));
            }
            export_model_text(&b.model)
        } else {
            match dynamic_lod_model(inp, b) {
                Some(m) => m,
                None if r.levels.first() == Some(&crate::rules::Level::Full) => export_model_text(&b.model),
                None => match lod.levels[0].clone() {
                    Some(m) => m,
                    None if r.reference_action.eq_ignore_ascii_case("Unchanged") => {
                        let path = crate::bases::model_path(&b.model);
                        if !(inp.resolver.exists)(&path) {
                            return Err(format!("dynamic reference {id:08X}: missing full-model fallback {path}"));
                        }
                        export_model_text(&b.model)
                    }
                    None => continue,
                },
            }
        };
        let top = if has_parent {
            parent_state(inp.reference_states, p.enable_parent_id, p.enable_parent_flags & 1 != 0)
                .map_err(|e| format!("dynamic reference {id:08X}: {e}"))?
        } else {
            (0, false, true)
        };
        out.push(DynamicRef {
            ref_id: id,
            base: base_id,
            grid: if replace && r.grid == Grid::None { Grid::FarFull } else { r.grid },
            // A replacement owns the original's local startup state; its parent is evaluated
            // by the DLL. Ordinary LOD also snapshots the existing parent's whole chain.
            initially_enabled: !disabled && (replace || !has_parent || top.2),
            enable_parent: if has_parent { top.0 } else { 0 },
            opposite_parent: top.1,
            large: inp.large_refs.contains(&id),
            persistent: p.flags & lodforge_esp::record::FLAG_PERSISTENT != 0,
            replace,
            model,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod parent_tests {
    use super::*;

    fn full_model_fixture(action: &str, present: bool, lod_paths: &[&str], flags: u32) -> Result<Vec<DynamicRef>, String> {
        fixture_with_model("object.nif", action, present, lod_paths, flags)
    }

    fn fixture_with_model(model: &str, action: &str, present: bool, lod_paths: &[&str], flags: u32) -> Result<Vec<DynamicRef>, String> {
        let mut world = Worldspace::default();
        world.refs.insert(1, lodforge_esp::Placement {
            base: 100, flags, position: [0.0; 3], rotation: [0.0; 3], scale: 1.0,
            has_scale: false, enable_parent: false, enable_parent_id: 0, enable_parent_flags: 0,
            cell: Some((0, 0)), offset: 0, plugin: 0,
        });
        let bases = HashMap::from([(100, Base { form_id: 100, signature: *b"DOOR", model: model.into(), ..Default::default() })]);
        let rules: Vec<_> = if action.is_empty() { Vec::new() } else { vec![crate::rules::Rule::parse(&format!("object,,,,Far LOD,{action},0"), "test").unwrap()] };
        let index = crate::lodmodels::LodIndex::scan(lod_paths.iter().copied(), &["meshes".into()], &[], &HashSet::new());
        let materials = HashMap::new();
        let owners = HashMap::from([(0, 0)]);
        let plugins = ["Test.esm".into()];
        let parallax = Default::default();
        let mesh_crc = crate::lodmodels::MeshCrcTable::default();
        let resolver = Resolver {
            rules: &rules, index: &index, parallax: &parallax, materials: &materials,
            plugins: &plugins, owners: &owners, read: &|_| None, exists: &|_| present, level32: false, mesh_crc: &mesh_crc,
        };
        build_dynamic(&ExportInput {
            world: &world, bases: &bases, materials: &materials, resolver: &resolver,
            rules: &rules, plugins: &plugins, owners: &owners, ignore_base_edid: &[],
            large_refs: &HashSet::new(), swaps: &HashMap::new(), xesp_lod: &HashSet::new(),
            tree_max_tilt: 30.0, tree_object_bounds: &[], windows: false, ignore_plugins: &HashSet::new(), ultra: false, reference_states: &HashMap::new(),
        })
    }

    #[test]
    fn unchanged_dynamic_lod_falls_back_to_existing_full_model() {
        let refs = full_model_fixture("Unchanged", true, &[], 0).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].model, "Meshes\\object.nif");
        assert!(full_model_fixture("Unchanged", false, &[], 0).unwrap_err().contains("missing full-model fallback"));
        assert!(full_model_fixture("Disable", true, &[], 0).unwrap().is_empty());
        let refs = full_model_fixture("Unchanged", true, &["meshes\\object_lod_0.nif"], 0).unwrap();
        assert_eq!(refs[0].model, "meshes\\object_lod_0.nif");
        let refs = full_model_fixture("Unchanged", true, &["meshes\\object_dyndolod_lod.nif", "meshes\\object_lod_0.nif"], 0).unwrap();
        assert_eq!(refs[0].model, "meshes\\object_dyndolod_lod.nif");
    }

    /// One TREE reference under DynDOLOD's High tree rule (3D model at LOD4, billboards beyond),
    /// with a billboard and the given LOD models installed.
    fn tree_lines(ultra: bool, lod_paths: &[&str]) -> (Vec<RefLine>, HashMap<u32, Skip>) {
        let mut world = Worldspace::default();
        world.refs.insert(0x801, lodforge_esp::Placement {
            base: 0x800, flags: 0, position: [100.0, 100.0, 0.0], rotation: [0.0; 3], scale: 1.0,
            has_scale: false, enable_parent: false, enable_parent_id: 0, enable_parent_flags: 0,
            cell: Some((0, 0)), offset: 0, plugin: 0,
        });
        let bases = HashMap::from([(0x800, Base { form_id: 0x800, signature: *b"TREE", model: "landscape\\trees\\testtree.nif".into(), ..Default::default() })]);
        let rules = vec![crate::rules::Rule::parse("tree,Level0,Billboard1,Billboard1,Far LOD,Unchanged,0", "test").unwrap()];
        let index = crate::lodmodels::LodIndex::scan(lod_paths.iter().copied(), &["meshes".into()], &[], &HashSet::new());
        let materials = HashMap::new();
        let owners = HashMap::from([(0, 0)]);
        let plugins = ["Test.esm".into()];
        let parallax = Default::default();
        let mesh_crc = crate::lodmodels::MeshCrcTable::default();
        let resolver = Resolver {
            rules: &rules, index: &index, parallax: &parallax, materials: &materials,
            plugins: &plugins, owners: &owners, read: &|_| None, exists: &|_| true, level32: false, mesh_crc: &mesh_crc,
        };
        build_lines(&ExportInput {
            world: &world, bases: &bases, materials: &materials, resolver: &resolver,
            rules: &rules, plugins: &plugins, owners: &owners, ignore_base_edid: &[],
            large_refs: &HashSet::new(), swaps: &HashMap::new(), xesp_lod: &HashSet::new(),
            tree_max_tilt: 30.0, tree_object_bounds: &[], windows: false, ignore_plugins: &HashSet::new(), ultra, reference_states: &HashMap::new(),
        })
    }

    #[test]
    fn ultra_keeps_trees_in_object_lod_with_billboard_fallback() {
        // Classic: the tree goes to the game's tree LOD.
        let (lines, skipped) = tree_lines(false, &[]);
        assert!(lines.is_empty());
        assert!(matches!(skipped.get(&0x801), Some(Skip::TreeLod)));
        // Ultra without a 3D model: the billboard at every level (TreeFullFallBack=1).
        let (lines, _) = tree_lines(true, &[]);
        assert_eq!(lines.len(), 1);
        let c = &lines[0].columns;
        assert!(!c[0].is_empty() && c[0] == c[1] && c[1] == c[2], "{c:?}");
        // Ultra with a 3D passthru model: the model at LOD4, billboards beyond.
        let (lines, _) = tree_lines(true, &["meshes\\dyndolod\\lod\\trees\\testtreepassthru_lod.nif"]);
        let c = &lines[0].columns;
        assert!(c[0].ends_with("testtreepassthru_lod.nif"), "{c:?}");
        assert_ne!(c[0], c[1]);
    }

    #[test]
    fn model_less_bases_are_skipped_not_failed() {
        // JK's Skyrim places persistent Is Full LOD collision boxes on CollisionMarker (no MODL):
        // the persistent full-LOD fallback must not turn them into a missing-model error.
        for model in ["", "  "] {
            assert!(fixture_with_model(model, "", false, &[], 0x10400).unwrap().is_empty());
            assert!(fixture_with_model(model, "Replace", false, &[], 0).unwrap().is_empty());
            assert!(fixture_with_model(model, "Unchanged", false, &[], 0).unwrap().is_empty());
        }
    }

    #[test]
    fn replacements_always_use_the_full_model() {
        let refs = full_model_fixture("Replace", true, &["meshes\\object_dyndolod_lod.nif", "meshes\\object_lod_0.nif"], 0).unwrap();
        assert_eq!(refs.len(), 1);
        assert!(refs[0].replace);
        assert_eq!(refs[0].model, "Meshes\\object.nif");
        assert!(refs[0].initially_enabled);
        assert!(!full_model_fixture("Replace", true, &[], FLAG_INITIALLY_DISABLED).unwrap()[0].initially_enabled);
        assert!(full_model_fixture("Replace", false, &[], 0).unwrap_err().contains("missing full model"));
    }

    #[test]
    fn persistent_full_lod_without_rules_keeps_distant_geometry() {
        assert!(full_model_fixture("", true, &[], 0).unwrap().is_empty());
        assert!(full_model_fixture("", true, &[], 0x10000).unwrap().is_empty());
        let refs = full_model_fixture("", true, &[], 0x10400).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].grid, crate::rules::Grid::FarFull);
        assert_eq!(refs[0].model, "Meshes\\object.nif");
        assert!(full_model_fixture("", false, &[], 0x10400).unwrap_err().contains("missing full model"));
        assert!(full_model_fixture("Disable", true, &[], 0x10400).unwrap().is_empty());
    }

    #[test]
    fn no_lod_rule_blocks_full_model_fallback_but_not_discovered_lod() {
        let rules = [
            crate::rules::Rule::parse("fxwaterfallmist,,,,,Unchanged,0", "test").unwrap(),
            crate::rules::Rule::parse("fxwaterfall,,,,Near LOD,Unchanged,0", "test").unwrap(),
        ];
        let selected = crate::rules::select_mask_rule(&rules, "effects\\fxwaterfallmistblast.nif", false, &|_| false).unwrap();
        assert_eq!(selected.grid, crate::rules::Grid::None);
        assert!(!selected.has_levels());
        let selected = crate::rules::select_mask_rule(&rules, "effects\\fxwaterfallmistblast.nif", false, &|_| true).unwrap();
        assert_eq!(selected.grid, crate::rules::Grid::NearLod);
    }

    #[test]
    fn base_updates_require_emitted_large_object_lod() {
        let line = |ref_id, base, columns| RefLine { ref_id, base, columns, ref_flags: 0, position: [0.0; 3], rotation: [0.0; 3], scale: 1.0, editor_id: String::new(), base_flags: 0, material: String::new(), full_model: String::new() };
        let model = || ["rock_lod.nif".into(), String::new(), String::new(), String::new()];
        let copy = crate::childworld::COPY_ID_BASE;
        let lines = [
            line(1, 100, model()),
            line(2, 100, model()),
            line(3, 200, model()),
            line(copy, 300, model()),
            line(4, 400, Default::default()),
        ];
        assert_eq!(large_object_bases(&lines, &HashSet::from([1, 2, 4, copy, 5])), HashSet::from([100]));
        assert!(large_object_bases(&[], &HashSet::from([1])).is_empty());
    }

    #[test]
    fn exterior_reference_can_follow_disabled_interior_marker() {
        let states = HashMap::from([(0x5eacd, ReferenceState { flags: FLAG_INITIALLY_DISABLED, ..Default::default() })]);
        assert_eq!(parent_state(&states, 0x5eacd, true), Ok((0x5eacd, true, true)));
        assert_eq!(parent_state(&states, 0x5eacd, false), Ok((0x5eacd, false, false)));
    }

    #[test]
    fn parent_chain_accumulates_opposites_without_depth_limit() {
        let mut states = HashMap::from([(100, ReferenceState::default())]);
        for id in 101..=140 {
            states.insert(id, ReferenceState { parent: id - 1, opposite: true, ..Default::default() });
        }
        assert_eq!(parent_state(&states, 140, true), Ok((100, true, false)));
        assert_eq!(parent_state(&states, 139, true), Ok((100, false, false)));
        assert_eq!(parent_state(&states, PLAYER_REF, true), Ok((PLAYER_REF, true, false)));
    }

    #[test]
    fn initial_snapshot_and_runtime_parent_are_distinct() {
        let states = HashMap::from([
            (100, ReferenceState { parent: 101, ..Default::default() }),
            (101, ReferenceState { flags: FLAG_INITIALLY_DISABLED, ..Default::default() }),
        ]);
        assert_eq!(parent_state(&states, 100, true), Ok((101, true, false)));
        assert_eq!(parent_state(&states, 101, true), Ok((101, true, true)));
        let states = HashMap::from([
            (100, ReferenceState { parent: 101, opposite: true, ..Default::default() }),
            (101, ReferenceState { flags: FLAG_INITIALLY_DISABLED, ..Default::default() }),
        ]);
        assert_eq!(parent_state(&states, 100, false), Ok((101, true, true)));
        let states = HashMap::from([
            (100, ReferenceState { flags: FLAG_INITIALLY_DISABLED, parent: 101, opposite: true }),
            (101, ReferenceState::default()),
        ]);
        assert_eq!(parent_state(&states, 100, false), Ok((101, true, false)));
    }

    #[test]
    fn missing_and_cyclic_parents_are_errors() {
        let states = HashMap::from([
            (100, ReferenceState { parent: 101, ..Default::default() }),
            (101, ReferenceState { parent: 100, ..Default::default() }),
        ]);
        assert!(parent_state(&states, 102, false).unwrap_err().contains("missing"));
        assert!(parent_state(&states, 100, false).unwrap_err().contains("cycle"));
    }
}
