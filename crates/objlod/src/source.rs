//! Base records and value formatting shared by the exports: the LOD-relevant parts of STAT /
//! TREE bases, material names, and xEdit's text formatting of rotations (`Core\wbLOD.pas`, MPL),
//! which LODGen parses back: degrees with 4 decimals.

use std::collections::HashMap;

use lodforge_esp::{LoadOrder, Sig, fields, winning_records};

/// The LOD-relevant parts of a STAT / TREE base record (winning override).
#[derive(Debug, Clone, Default)]
pub struct Base {
    pub signature: [u8; 4],
    pub editor_id: String,
    pub flags: u32,
    pub model: String,
    /// MNAM LOD meshes, levels 0..3 (as stored, without `Meshes\`).
    pub lod: [String; 4],
    pub has_mnam: bool,
    /// DNAM material (MATO global form id), 0 for none.
    pub material: u32,
    /// DNAM "Considered Snow" (SSE).
    pub considered_snow: Option<u8>,
}

pub const REFR_IS_FULL_LOD: u32 = 0x10000;

fn zstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

/// Winning STAT and TREE records.
pub fn read_bases(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, Base>> {
    let mut out = HashMap::new();
    for sig in [*b"STAT", *b"TREE"] {
        for (id, (pi, r)) in winning_records(lo, Sig(sig))? {
            let lp = &lo.plugins[pi as usize];
            let data = r.data()?;
            let mut b = Base { signature: sig, flags: r.flags, ..Default::default() };
            for f in fields(&data) {
                match &f.sig.0 {
                    b"EDID" => b.editor_id = f.zstring(),
                    b"MODL" => b.model = f.zstring(),
                    b"DNAM" if f.data.len() >= 8 => {
                        let m = u32::from_le_bytes([f.data[4], f.data[5], f.data[6], f.data[7]]);
                        b.material = if m == 0 { 0 } else { lp.global_form_id(m).unwrap_or(0) };
                        b.considered_snow = f.data.get(8).copied();
                    }
                    b"MNAM" => {
                        b.has_mnam = true;
                        for (k, l) in b.lod.iter_mut().enumerate() {
                            if let Some(s) = f.data.get(k * 260..(k + 1) * 260) {
                                *l = zstr(s);
                            }
                        }
                    }
                    _ => {}
                }
            }
            out.insert(id, b);
        }
    }
    Ok(out)
}

/// Delphi `SingleSameValue`.
fn single_same(a: f64, b: f64) -> bool {
    const RES: f64 = 0.000_000_5;
    let (a, b) = (f64::from(a as f32), f64::from(b as f32));
    (a - b).abs() <= (a.abs().min(b.abs()) * RES).max(RES)
}

/// xEdit's `RoundToEx(v, -digits)` = `Round(v / IntPower(10, -digits)) * IntPower(...)`, where
/// the factor ends up in single precision (the golden exports only fit with it), with banker's
/// rounding.
fn round_to(v: f64, digits: i32) -> f64 {
    let f = f64::from(10f32.powi(-digits));
    (v / f).round_ties_even() * f
}

/// xEdit's rotation text (radians normalised to 0..2pi with a single-precision 2pi, degrees
/// rounded to 4 decimals) parsed back as LODGen does. Matches 99.7 % of the golden Tamriel export's
/// angles exactly; the rest differ by 0.0001 degree.
pub fn angle_as_lodgen(rad: f32) -> f32 {
    let two_pi = std::f64::consts::TAU;
    let tp = f64::from(std::f32::consts::TAU);
    let mut r = f64::from(rad);
    if (r / two_pi).abs() > 100.0 {
        r -= r.signum() * two_pi * ((r / two_pi).abs() - 100.0).trunc();
        if (r / two_pi).abs() > 101.0 {
            return f32::NAN;
        }
    }
    while r < 0.0 {
        r += tp;
    }
    while r > two_pi {
        r -= tp;
    }
    if single_same(r, 0.0) || r < 0.0 || single_same(r, two_pi) || r > two_pi {
        r = 0.0;
    }
    let deg = round_to(r * (180.0 / std::f64::consts::PI), 4);
    format!("{deg:.4}").parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xedit_formatting() {
        assert_eq!(angle_as_lodgen(std::f32::consts::FRAC_PI_4 * 3.0), 135.0);
        assert_eq!(angle_as_lodgen(-0.1), "354.2704".parse::<f32>().unwrap());
    }
}
