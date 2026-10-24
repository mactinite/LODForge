//! Hashes and quantisation (docs/runtime/instance-formats.md sections 1 and 4).

/// MurmurHash3's 32-bit finaliser.
pub fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

/// FNV-1a, 32 bits.
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Hash of a reference by plugin file name and local form id (load-order independent).
pub fn ref_hash(plugin: &str, form: u32) -> u32 {
    fmix32(fnv1a32(plugin.to_ascii_lowercase().as_bytes()) ^ fmix32(form ^ 0x9E37_79B9))
}

/// Hash of a grass instance by cell and cell-relative quantised position.
pub fn grass_hash(cell_x: i16, cell_y: i16, qx: u16, qy: u16) -> u32 {
    let c = u32::from(cell_x as u16) | (u32::from(cell_y as u16) << 16);
    fmix32(fmix32(c) ^ (u32::from(qx) | (u32::from(qy) << 16)) ^ 0x5BD1_E995)
}

/// Hash of a synthetic reference (no plugin): its position bits.
pub fn position_hash(p: [f32; 3]) -> u32 {
    fmix32(fmix32(fmix32(p[0].to_bits()) ^ p[1].to_bits()) ^ p[2].to_bits())
}

pub const CELL: f32 = 4096.0;

/// The cell of a world coordinate.
pub fn cell_of(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

/// Cell-relative position in 1/16 units.
pub fn quantize_rel(v: f32, cell: i32) -> u16 {
    let q = ((f64::from(v) - f64::from(cell) * 4096.0) * 16.0).round();
    q.clamp(0.0, 65535.0) as u16
}

pub fn dequantize_rel(q: u16, cell: i32) -> f32 {
    (f64::from(cell) * 4096.0 + f64::from(q) / 16.0) as f32
}

/// Radians to the u16 angle encoding.
pub fn angle_u16(rad: f32) -> u16 {
    let turns = f64::from(rad) / std::f64::consts::TAU;
    ((turns * 65536.0).round() as i64).rem_euclid(65536) as u16
}

pub fn angle_rad(a: u16) -> f32 {
    (f64::from(a) * std::f64::consts::TAU / 65536.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors() {
        assert_eq!(fmix32(0), 0);
        assert_eq!(fmix32(1), 0x514E_28B7);
        assert_eq!(fnv1a32(b""), 0x811C_9DC5);
        assert_eq!(fnv1a32(b"a"), 0xE40C_292C);
        // Pinned values (docs/runtime/instance-formats.md section 8).
        assert_eq!(fnv1a32(b"skyrim.esm"), 0xE330_F2F3, "{:08X}", fnv1a32(b"skyrim.esm"));
        assert_eq!(ref_hash("Skyrim.esm", 0x0001_2345), ref_hash("skyrim.esm", 0x0001_2345));
        assert_eq!(ref_hash("Skyrim.esm", 0x0001_2345), 0x997D_E556, "{:08X}", ref_hash("Skyrim.esm", 0x0001_2345));
        assert_eq!(grass_hash(-3, 7, 100, 200), 0x6D7E_02AF, "{:08X}", grass_hash(-3, 7, 100, 200));
        assert_eq!(angle_u16(std::f32::consts::PI), 0x8000);
        assert_eq!(angle_u16(-std::f32::consts::FRAC_PI_2), 0xC000);
        assert_eq!(angle_u16(std::f32::consts::TAU), 0);
        assert_eq!(quantize_rel(4096.0 * 3.0 + 100.5, 3), 1608);
        assert_eq!(quantize_rel(-1.0, -1), 65520);
        assert_eq!(quantize_rel(4095.999, 0), 65535);
        assert_eq!(cell_of(-0.5), -1);
        assert_eq!(dequantize_rel(1608, 3), 4096.0 * 3.0 + 100.5);
    }
}
