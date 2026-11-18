//! Season swaps of base records and the atlas normal-map pairing, shared by the tree, impostor
//! and atlas stages.

use std::collections::HashMap;

use anyhow::Result;

use lodforge_objlod::source::read_bases;
use lodforge_seasons::{Season, load_swaps};

use crate::profile::Profile;

/// The profile's season swaps, loaded once for all seasons.
pub struct ExportInputs {
    pub swaps: lodforge_seasons::SeasonSwaps,
}

pub fn load_inputs(p: &Profile) -> Result<ExportInputs> {
    let lo = &p.load_order;
    let bases = read_bases(lo)?;
    let edids: HashMap<String, u32> = bases.iter().map(|(id, b)| (b.editor_id.to_ascii_lowercase(), *id)).collect();
    let swaps = load_swaps(&p.vfs, lo, &edids)?;
    Ok(ExportInputs { swaps })
}

/// Static and tree swaps of one season.
pub fn base_swaps(inputs: &ExportInputs, season: Season) -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    if let Some(sections) = inputs.swaps.swaps.get(&season) {
        for (name, map) in sections {
            if name.eq_ignore_ascii_case("statics") || name.eq_ignore_ascii_case("trees") {
                out.extend(map.iter().map(|(a, b)| (*a, *b)));
            }
        }
    }
    out
}

pub fn season_from_code(code: &str) -> Option<Season> {
    Season::ALL.into_iter().find(|s| s.code().eq_ignore_ascii_case(code))
}

/// The normal map xEdit pairs with an atlas diffuse (`_d` -> `_n`, else `_n` appended, else the
/// default flat normal).
pub fn normal_for(diffuse: &str, exists: impl Fn(&str) -> bool) -> String {
    let stem = |s: &str| s.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_else(|| s.to_string());
    let lower = diffuse.to_lowercase();
    let mut n = if lower.contains("_d.dds") {
        let i = lower.find("_d.dds").unwrap();
        let cand = format!("{}_n.dds{}", &diffuse[..i], &diffuse[i + 6..]);
        if exists(&cand) { cand } else { format!("{}_n.dds", stem(diffuse)) }
    } else {
        format!("{}_n.dds", stem(diffuse))
    };
    if !exists(&n) {
        n = "textures\\default_n.dds".into();
    }
    n
}
