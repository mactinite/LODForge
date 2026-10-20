//! The content of Occlusion.esp: the winning WRLD and CELL overrides of a worldspace with the
//! computed TVDT (and MHDT where the record had none), as docs/re/occlusion-spec.md section 7
//! describes the golden plugin. The bytes are produced by [`crate::plugin::PluginBuilder`].

use lodforge_esp::{LoadOrder, Worldspace};

use crate::plugin::{CellContent, PluginBuilder, Rec, WorldContent};

/// xEdit's CELL definition order (`wbDefinitionsTES5.pas`, record `CELL`): the order a copied
/// record's subrecords are written in.
pub const CELL_ORDER: [&[u8; 4]; 26] = [
    b"EDID", b"FULL", b"DATA", b"XCLC", b"XCLL", b"TVDT", b"MHDT", b"LTMP", b"LNAM", b"XCLW", b"XNAM", b"XCLR", b"XLCN", b"XWCN", b"XWCS", b"XWCU", b"XCWT", b"XOWN", b"XRNK", b"XILL", b"XWEM", b"XCCM", b"XCAS", b"XEZN", b"XCMO", b"XCIM",
];

/// An empty Occlusion.esp: ESL-flagged (overrides only), HEDR 1.71, next object id 0x800.
pub fn plugin_builder() -> PluginBuilder {
    let mut pb = PluginBuilder::new(0x800);
    pb.esl = true;
    pb.author = "LODForge".into();
    pb.description = "Generated plugin for TVDT occlusion data".into();
    pb.incc = Some(0);
    pb
}

/// The string with `id` in a `.strings` table (`u32 count`, `u32 data size`, `count` x
/// (`u32 id`, `u32 offset` into the data block), then the zero-terminated strings).
pub fn strings_lookup(data: &[u8], id: u32) -> Option<String> {
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let count = u32_at(0)? as usize;
    let base = 8 + count * 8;
    for k in 0..count {
        if u32_at(8 + k * 8)? == id {
            let start = base.checked_add(u32_at(12 + k * 8)? as usize)?;
            let rest = data.get(start..)?;
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            return Some(rest[..end].iter().map(|&b| b as char).collect());
        }
    }
    None
}

fn zstring(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
    v.push(0);
    v
}

/// Replaces the string ids of a record copied from a localized plugin by the strings themselves:
/// Occlusion.esp is not flagged Localized, so its FULL subrecords hold plain text.
fn delocalize(rec: &mut Rec, lo: &LoadOrder, plugin: u16, text: &dyn Fn(&str, u32) -> Option<String>) {
    let lp = &lo.plugins[usize::from(plugin)];
    if !lp.plugin.header.is_localized() {
        return;
    }
    for (sig, data) in &mut rec.fields {
        if sig == b"FULL" && data.len() == 4 {
            let id = u32::from_le_bytes(data[..4].try_into().unwrap());
            *data = zstring(&text(&lp.plugin.name, id).unwrap_or_default());
        }
    }
}

fn rank(sig: &[u8; 4]) -> usize {
    CELL_ORDER.iter().position(|s| *s == sig).unwrap_or(CELL_ORDER.len())
}

/// The worldspace's override records for the plugin: the winning WRLD without its large
/// references and max-height data, and for every `(grid, tvdt)` a copy of the winning CELL with
/// that TVDT, an MHDT from `mhdt(grid)` when the record had none, and its subrecords in
/// xEdit's order. `text(plugin, id)` resolves localized strings. `None` when the worldspace has
/// no WRLD record in the load order.
pub fn world_content(lo: &LoadOrder, world: &Worldspace, cells: &[((i32, i32), Vec<u8>)], text: &dyn Fn(&str, u32) -> Option<String>, mhdt: &dyn Fn((i32, i32)) -> Option<Vec<u8>>) -> Option<WorldContent> {
    let wr = world.world_record?;
    let mut w = Rec::copy(lo, wr)?;
    w.fields.retain(|(s, _)| s != b"RNAM" && s != b"MHDT");
    delocalize(&mut w, lo, wr.plugin, text);
    let mut out = Vec::with_capacity(cells.len());
    for (grid, tvdt) in cells {
        let Some(&rr) = world.cell_records.get(grid) else { continue };
        let Some(mut c) = Rec::copy(lo, rr) else { continue };
        delocalize(&mut c, lo, rr.plugin, text);
        match c.fields.iter_mut().find(|(s, _)| s == b"TVDT") {
            Some(f) => f.1 = tvdt.clone(),
            None => c.fields.push((*b"TVDT", tvdt.clone())),
        }
        if !c.fields.iter().any(|(s, _)| s == b"MHDT")
            && let Some(m) = mhdt(*grid)
        {
            c.fields.push((*b"MHDT", m));
        }
        debug_assert!(c.id_at.is_empty(), "copied records carry no extra id positions");
        c.fields.sort_by_key(|(s, _)| rank(s));
        out.push(CellContent { grid: *grid, cell: c, temporary: Vec::new() });
    }
    Some(WorldContent { world: w, persistent_cell: None, persistent: Vec::new(), cells: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_table_lookup() {
        // Two strings: id 7 -> "Riverwood", id 9 -> "".
        let payload = b"Riverwood\0\0";
        let mut t = Vec::new();
        t.extend_from_slice(&2u32.to_le_bytes());
        t.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        t.extend_from_slice(&7u32.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&9u32.to_le_bytes());
        t.extend_from_slice(&10u32.to_le_bytes());
        t.extend_from_slice(payload);
        assert_eq!(strings_lookup(&t, 7).as_deref(), Some("Riverwood"));
        assert_eq!(strings_lookup(&t, 9).as_deref(), Some(""));
        assert_eq!(strings_lookup(&t, 8), None);
        assert_eq!(strings_lookup(&t[..5], 7), None);
    }

    #[test]
    fn cell_order_is_xedits() {
        assert!(rank(b"EDID") < rank(b"DATA") && rank(b"XCLC") < rank(b"TVDT") && rank(b"TVDT") < rank(b"MHDT") && rank(b"MHDT") < rank(b"XCLW") && rank(b"XCLR") < rank(b"XLCN"));
        assert_eq!(rank(b"ZZZZ"), CELL_ORDER.len());
    }
}
