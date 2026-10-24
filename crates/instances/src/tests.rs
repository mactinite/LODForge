use half::f16;

use crate::container::{Container, Kind, WriteOptions};
use crate::grass::{GrassCell, GrassFile, GrassType, Instance};
use crate::hash::{grass_hash, ref_hash};
use crate::statics::{Mesh, StaticsFile};
use crate::trees::{TreeFile, TreeType};

fn common(world: &str, season: &str) -> crate::Common {
    crate::Common { world: world.into(), season: season.into(), generator: "test".into(), plugins: Vec::new(), params: "min_copies=8".into() }
}

fn trees() -> TreeFile {
    let mut f = TreeFile { common: common("Tamriel", ""), ..Default::default() };
    let p = f.common.plugin_index("Skyrim.esm");
    f.types.push(TreeType { tree_record: true, bases: vec![(p, 0x6F7DC)], name: "treepineforest01_1234abcd".into(), model_crc32: 0xDEADBEEF, bounds: [-300, -300, -20, 300, 300, 1800], references: 0, model: r"meshes\landscape\trees\treepineforest01.nif".into(), lod4: r"meshes\dyndolod\lod\trees\treepineforest01passthru_lod.nif".into() });
    for i in 0..50u32 {
        let x = -6000.0 + i as f32 * 397.25;
        f.add([x, 1234.5 + i as f32, 100.0 + i as f32], i as f32 * 0.3 - 3.0, 0.8 + i as f32 / 100.0, 0, if i % 3 == 0 { "Skyrim.esm" } else { "Mod.esl" }, 0x800 + i).unwrap();
    }
    f.finish();
    f
}

#[test]
fn trees_round_trip() {
    let f = trees();
    assert_eq!(f.types[0].references, 50);
    for opts in [WriteOptions::default(), WriteOptions { zstd: Some(3), crc: true }] {
        let bytes = f.to_bytes(opts).unwrap();
        let back = TreeFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, f);
        let c = Container::parse(&bytes, Kind::Trees).unwrap();
        assert_eq!(c.header.total_records, 50);
        assert_eq!(c.header.record_size, 24);
        assert_eq!((c.header.min, c.header.max), ((-2, 0), (3, 0)));
        assert!(c.find(0, -1, 0).is_some());
        assert!(c.find(0, 9, 9).is_none());
        // Index sorted by (level, y, x), payloads 4-byte aligned.
        assert!(c.index.windows(2).all(|w| w[0].key() < w[1].key()));
        assert!(c.index.iter().all(|e| e.offset % 4 == 0));
    }
    // Positions and angles come back within the quantisation.
    for (&cell, recs) in &f.cells {
        for r in recs {
            let p = r.position(cell);
            let i = (r.form - 0x800) as f32;
            assert!((p[0] - (-6000.0 + i * 397.25)).abs() <= 1.0 / 32.0);
            assert!((p[1] - (1234.5 + i)).abs() <= 1.0 / 32.0);
            let yaw = (i * 0.3 - 3.0).rem_euclid(std::f32::consts::TAU);
            assert!((r.yaw_rad() - yaw).abs() < 1e-3, "{} {}", r.yaw_rad(), yaw);
            assert_eq!(r.hash, ref_hash(&f.common.plugins[usize::from(r.plugin)], r.form));
        }
        assert!(recs.windows(2).all(|w| w[0].hash <= w[1].hash));
    }
}

#[test]
fn rejects_other_major_and_kind() {
    let mut bytes = trees().to_bytes(WriteOptions::default()).unwrap();
    assert!(StaticsFile::from_bytes(&bytes).is_err());
    bytes[4] = 2;
    assert!(matches!(TreeFile::from_bytes(&bytes), Err(crate::Error::Version(2, 0))));
}

#[test]
fn corrupt_crc_is_detected() {
    let mut bytes = trees().to_bytes(WriteOptions { zstd: None, crc: true }).unwrap();
    let n = bytes.len();
    bytes[n - 40] ^= 0x55;
    assert!(TreeFile::from_bytes(&bytes).is_err());
}

#[test]
fn statics_round_trip() {
    let mut f = StaticsFile::new(common("Tamriel", "WIN"));
    let pine = f.mesh(r"meshes\dyndolod\lod\trees\treepineforest01passthru_lod.nif", || Mesh { bounds: [-1.0, -2.0, 0.0, 1.0, 2.0, 30.0], flags: crate::statics::MESH_TREE | crate::statics::MESH_PASSTHRU, triangles: 120, model: String::new() });
    let rock = f.mesh(r"meshes\lod\rocks\rock01_lod.nif", Mesh::default);
    assert_eq!(f.mesh(r"MESHES\lod\rocks\rock01_lod.nif", Mesh::default), rock);
    for i in 0..20u32 {
        f.add(4, (-4, 8), [i as f32 * 100.0, 5.0, -3.0], [0.1, -0.2, i as f32], 1.5, if i % 2 == 0 { pine } else { rock }, Some("Skyrim.esm"), 0x1000 + i);
    }
    f.add(8, (-8, 8), [98400.0, -237948.0, -7100.0], [0.0; 3], 1.0, rock, None, 0);
    f.finish();
    let bytes = f.to_bytes(WriteOptions { zstd: Some(1), crc: true }).unwrap();
    let back = StaticsFile::from_bytes(&bytes).unwrap();
    assert_eq!(back.meshes, f.meshes);
    assert_eq!(back.blocks, f.blocks);
    assert_eq!(back.common.file_name(crate::statics::EXT), "Tamriel.WIN.statics.bin");
    let recs = &back.blocks[&(4, -4, 8)];
    assert!(recs.windows(2).all(|w| (w[0].mesh, w[0].hash) <= (w[1].mesh, w[1].hash)));
    assert_eq!(recs[0].scale, f16::from_f32(1.5));
    let c = Container::parse(&bytes, Kind::Statics).unwrap();
    assert_eq!(c.index.iter().map(|e| e.level).collect::<Vec<_>>(), vec![4, 8]);
    assert_eq!(back.blocks[&(8, -8, 8)][0].plugin, crate::statics::NO_PLUGIN);
}

fn grass_instances(n: usize) -> Vec<Instance> {
    // Deterministic pseudo-random instances inside cell (-3, 7).
    let mut s = 12345u32;
    let mut next = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / (1u32 << 24) as f32
    };
    (0..n)
        .map(|_| {
            let tilt = next() * 0.6 - 0.3;
            let n = [tilt, -tilt / 2.0, (1.0 - tilt * tilt * 1.25).sqrt()];
            Instance { pos: [-3.0 * 4096.0 + next() * 4096.0, 7.0 * 4096.0 + next() * 4096.0, -500.0 + next() * 3000.0], yaw: next() * std::f32::consts::TAU, scale: 0.6 + next(), normal: n, type_: (next() * 5.0) as u16 * 3 }
        })
        .collect()
}

#[test]
fn grass_cell_ranks_and_quantises() {
    let inst = grass_instances(5000);
    let cell = GrassCell::build(-3, 7, &inst).unwrap();
    assert_eq!(cell.records.len(), 5000);
    assert_eq!(cell.prefix[0], 5000);
    assert!(cell.prefix.windows(2).all(|w| w[0] >= w[1]));
    // Roughly halves per level.
    for k in 1..8 {
        let expect = 5000.0 / f64::from(1u32 << k);
        assert!((f64::from(cell.prefix[k]) - expect).abs() < 4.0 * expect.sqrt() + 2.0, "level {k}: {}", cell.prefix[k]);
    }
    let hashes: Vec<u32> = cell.records.iter().map(|r| grass_hash(-3, 7, r.qx, r.qy)).collect();
    assert!(hashes.windows(2).all(|w| w[0] <= w[1]));
    for k in 0..8 {
        let n = cell.prefix[k] as usize;
        assert!(hashes[..n].iter().all(|&h| u64::from(h) < 1u64 << (32 - k)));
        assert!(hashes[n..].iter().all(|&h| u64::from(h) >= 1u64 << (32 - k)));
    }
    // Every input comes back within the quantisation (match by hash order is lost; compare sets).
    let by_q: std::collections::HashMap<(u16, u16), &crate::grass::GrassRecord> = cell.records.iter().map(|r| ((r.qx, r.qy), r)).collect();
    for i in &inst {
        let q = (crate::hash::quantize_rel(i.pos[0], -3), crate::hash::quantize_rel(i.pos[1], 7));
        let r = by_q[&q];
        let a = cell.position(r);
        assert!((a[0] - i.pos[0]).abs() <= 1.0 / 32.0 + 1e-3 && (a[1] - i.pos[1]).abs() <= 1.0 / 32.0 + 1e-3);
        assert!((a[2] - i.pos[2]).abs() <= 3000.0 / 65535.0 + 1e-3);
        assert!((r.scale_f32() - i.scale).abs() <= 1.0 / 128.0 + 1e-6);
    }
    let r = &cell.records[0];
    let n = r.normal();
    assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) - 1.0).abs() < 1e-5);
    assert_eq!(cell.types.len(), 5);
}

#[test]
fn grass_file_round_trip() {
    let mut f = GrassFile { common: common("Tamriel", "AUT"), ..Default::default() };
    f.common.plugin_index("Skyrim.esm");
    for k in 0..15u16 {
        f.types.push(GrassType { plugin: 0, flags: k % 8, form: 0x1000 + u32::from(k), wave_period: 1.5, model: format!(r"meshes\landscape\grass\grass{k:02}.nif") });
    }
    let cell = GrassCell::build(-3, 7, &grass_instances(3000)).unwrap();
    let other = GrassCell::build(-3, 7, &grass_instances(10)).unwrap();
    let other = GrassCell { x: 4, ..other };
    f.cells.insert((-3, 7), cell);
    f.cells.insert((4, 7), other);
    for opts in [WriteOptions::default(), WriteOptions { zstd: Some(9), crc: true }] {
        let bytes = f.to_bytes(opts).unwrap();
        let back = GrassFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, f);
        let c = Container::parse(&bytes, Kind::Grass).unwrap();
        assert_eq!(c.header.total_records, 3010);
        let e = c.find(0, -3, 7).unwrap();
        assert_eq!(e.raw_size as usize, f.cells[&(-3, 7)].header_size() + 3000 * 10);
    }
}

#[test]
fn tilt_and_yaw_encoding() {
    let inst = [Instance { pos: [10.0, 10.0, 0.0], yaw: std::f32::consts::FRAC_PI_2, scale: 1.25, normal: [0.0, 0.0, 1.0], type_: 0 }, Instance { pos: [20.0, 10.0, 0.0], yaw: -0.1, scale: 5.0, normal: [-1.0, 0.0, 0.0], type_: 0 }];
    let c = GrassCell::build(0, 0, &inst).unwrap();
    let up = c.records.iter().find(|r| r.qx == 160).unwrap();
    assert_eq!(up.tilt, 0);
    assert_eq!(up.yaw, 64);
    assert_eq!(up.scale, 80);
    let side = c.records.iter().find(|r| r.qx == 320).unwrap();
    assert_eq!(side.normal()[0], -1.0);
    assert_eq!(side.scale, 255);
    assert_eq!(side.yaw, 252);
    // One record z: z_step falls back to 1 for a flat cell.
    assert_eq!(c.z_step, 1.0);
}

#[test]
fn cgid_conversion() {
    use lodforge_grass::cgid::{CellCache, Group, TypeCache};
    let h = |v: f32| f16::from_f32(v).to_bits();
    // Cell (-13, 5): the 12x12 block origin is (-12*4096, 0).
    let mut inst = [0u16; 16];
    inst[0] = h(-100.0);
    inst[1] = h(2000.0);
    inst[2] = h(-250.0);
    // r = Rz(90 deg): r0=0 r1=-1 r2=0 / r3=1 r4=0 r5=0 / r6=0 r7=0 r8=1, stored r0 r1 r2 r7 r3 r4 r5 r8 r6.
    for (k, v) in [0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0].into_iter().enumerate() {
        inst[4 + k] = h(v);
    }
    inst[13] = h(0.25);
    let t = TypeCache { gras: 0, model: r"landscape\grass\g.nif".into(), wave_period: 1.0, form_id: 0x0101_2345, fits_to_slope: false, uniform_scaling: true, vertex_lighting: false, groups: vec![Group { center: [0.0; 3], half_extent: [0.0; 3], instances: vec![inst] }] };
    let cache = CellCache { types: vec![t] };
    let out = crate::grass::instances_from_cgid((-13, 5), &cache, |_| Some(7));
    assert_eq!(out.len(), 1);
    let i = out[0];
    assert_eq!(i.pos, [-12.0 * 4096.0 - 100.0, 2000.0, -250.0]);
    assert!((i.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
    assert_eq!(i.scale, 1.25);
    assert_eq!(i.normal, [0.0, 0.0, 1.0]);
    assert_eq!(i.type_, 7);
}

/// A one-record trees file, byte for byte (docs/runtime/instance-formats.md section 8).
#[test]
fn golden_trees_bytes() {
    let mut f = TreeFile { common: crate::Common { world: "W".into(), season: String::new(), generator: "g".into(), plugins: Vec::new(), params: String::new() }, ..Default::default() };
    f.types.push(TreeType { tree_record: true, bases: vec![(0, 0x10)], name: "m_0".into(), model_crc32: 0x1234_5678, bounds: [-1, -2, -3, 4, 5, 6], references: 0, model: "m".into(), lod4: String::new() });
    f.common.plugin_index("Skyrim.esm");
    f.add([4096.0 + 1.0, -0.5, 10.0], std::f32::consts::PI, 1.0, 0, "Skyrim.esm", 0x0001_2345).unwrap();
    let bytes = f.to_bytes(WriteOptions { zstd: None, crc: true }).unwrap();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let expect = GOLDEN_TREES;
    assert_eq!(hex, expect, "\n{hex}");
}

const GOLDEN_TREES: &str = "4c4654520100000040000000000000001800000001000000c0000000000000004000000000000000680000000100ffff0100ffff7952ff7e010000000000000057524c44080000000100570000010067504c554710000000010000000a00536b7972696d2e65736d5041524d0200000000000000545459502c0000000100000078563412fffffefffdff040005000600010000000100010003006d5f3001006d00000000100000001000f8ff000020410080003c000000004523010056e57d990100ffff00000000a80000000000000018000000180000000100000074a1d2c5";
