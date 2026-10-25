use half::f16;

use crate::container::{Container, Kind, WriteOptions};
use crate::grass::{GrassCell, GrassFile, GrassType, Instance};
use crate::hash::{grass_hash, ref_hash};
use crate::statics::{Mesh, StaticsFile};
use crate::trees::{TreeFile, TreeType};

fn common(world: &str, season: &str) -> crate::Common {
    crate::Common { world: world.into(), season: season.into(), generator: "test".into(), plugins: Vec::new(), params: "min_copies=8".into(), token: String::new() }
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
    assert!(matches!(TreeFile::from_bytes(&bytes), Err(crate::Error::Version(2, 7))));
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
    let pine = f.mesh(r"meshes\dyndolod\lod\trees\treepineforest01passthru_lod.nif", || Mesh { bounds: [-1.0, -2.0, 0.0, 1.0, 2.0, 30.0], flags: crate::statics::MESH_TREE | crate::statics::MESH_PASSTHRU, triangles: 120, model: String::new(), textures: Vec::new(), classes: Vec::new() });
    let rock = f.mesh(r"meshes\lod\rocks\rock01_lod.nif", Mesh::default);
    let snowy = f.mesh_variant("rock-variant", || Mesh { model: r"meshes\lod\rocks\rock01_lod.nif".into(), flags: crate::statics::MESH_LARGE_REF, textures: vec![(r"textures\a.dds".into(), String::new())], classes: vec![1, 2], ..Default::default() });
    assert_ne!(snowy, rock);
    assert_eq!(f.mesh_variant("rock-variant", Mesh::default), snowy);
    assert_eq!(f.mesh(r"MESHES\lod\rocks\rock01_lod.nif", Mesh::default), rock);
    for i in 0..20u32 {
        f.add(4, (-4, 8), [i as f32 * 100.0, 5.0, -3.0], [0.1, -0.2, i as f32], 1.5, if i % 2 == 0 { pine } else { rock }, Some("Skyrim.esm"), 0x1000 + i);
    }
    f.add(8, (-8, 8), [98400.0, -237948.0, -7100.0], [0.0; 3], 1.0, rock, None, 0);
    f.pack_meshes = vec![3, crate::meshpack::NO_MESH];
    f.finish();
    let bytes = f.to_bytes(WriteOptions { zstd: Some(1), crc: true }).unwrap();
    let back = StaticsFile::from_bytes(&bytes).unwrap();
    assert_eq!(back.meshes, f.meshes);
    assert_eq!(back.pack_meshes, f.pack_meshes);
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
    f.pack = (0..15u32).map(|k| crate::grass::GrassTypePack { tiers: [3 * k, 3 * k + 1, if k == 2 { crate::meshpack::NO_MESH } else { 3 * k + 2 }], albedo: [90, 110, 40, 200], height: 40.0 + k as f32 }).collect();
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
    let mut f = TreeFile { common: crate::Common { world: "W".into(), season: String::new(), generator: "g".into(), plugins: Vec::new(), params: String::new(), token: String::new() }, ..Default::default() };
    f.types.push(TreeType { tree_record: true, bases: vec![(0, 0x10)], name: "m_0".into(), model_crc32: 0x1234_5678, bounds: [-1, -2, -3, 4, 5, 6], references: 0, model: "m".into(), lod4: String::new() });
    f.common.plugin_index("Skyrim.esm");
    f.add([4096.0 + 1.0, -0.5, 10.0], std::f32::consts::PI, 1.0, 0, "Skyrim.esm", 0x0001_2345).unwrap();
    let bytes = f.to_bytes(WriteOptions { zstd: None, crc: true }).unwrap();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let expect = GOLDEN_TREES;
    assert_eq!(hex, expect, "\n{hex}");
}

const GOLDEN_TREES: &str = "4c4654520100070040000000000000001800000001000000c0000000000000004000000000000000680000000100ffff0100ffff7952ff7e010000000000000057524c44080000000100570000010067504c554710000000010000000a00536b7972696d2e65736d5041524d0200000000000000545459502c0000000100000078563412fffffefffdff040005000600010000000100010003006d5f3001006d00000000100000001000f8ff000020410080003c000000004523010056e57d990100ffff00000000a80000000000000018000000180000000100000074a1d2c5";

#[test]
fn tree_meshes_round_trip() {
    use crate::trees::{MeshShape, TreeMesh};
    let mut f = trees();
    f.pack_meshes = vec![7];
    f.meshes = vec![TreeMesh { model: r"meshes\lod\lodforge\trees\treepineforest01_lod.nif".into(), bounds: [-1.0, -2.0, 0.0, 1.0, 2.0, 900.0], triangles: 420, generated: true, passthru: false, shapes: vec![MeshShape { name: "crown".into(), diffuse: r"textures\landscape\trees\pine.dds".into(), normal: r"textures\landscape\trees\pine_n.dds".into(), has_alpha: true, alpha_flags: 4844, alpha_threshold: 128, vertex_colors: true, triangles: 400 }, MeshShape { name: "trunk".into(), triangles: 20, ..Default::default() }] }];
    let bytes = f.to_bytes(WriteOptions { zstd: Some(3), crc: true }).unwrap();
    let back = TreeFile::from_bytes(&bytes).unwrap();
    assert_eq!(back.meshes, f.meshes);
    assert_eq!(back, f);
}

#[test]
fn dyn_round_trip() {
    use crate::dynlod::*;
    let mut c = common("Tamriel", "");
    c.token = "ABC123".into();
    let mut f = DynFile::new(c);
    f.header = DynHeader { world: FormRef { plugin: 0, form: 0x3C }, south_west: (-96, -96), near_grid: 11, far_grid: 21, glow_multiple: 1.75, lod_world: "Tamriel".into(), flags: HDR_LARGE_REFS_INSTANCED };
    f.common.plugin_index("Skyrim.esm");
    let m = f.model("a", || DynModel { flags: MODEL_FULL, pack: [3, NO_MESH, 4, NO_MESH], bounds: [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0], model: r"meshes\a.nif".into(), lod: [r"meshes\lod\a_lod_0.nif".into(), String::new(), String::new()], textures: [Vec::new(), vec![(r"textures\a.dds".into(), r"textures\a_n.dds".into())], Vec::new(), Vec::new()], classes: [Vec::new(), vec![1, 0x81], Vec::new(), Vec::new()] });
    assert_eq!(f.model("a", DynModel::default), m);
    f.add([5000.0, -10.0, 30.0], [0.1, 0.0, 3.0], 1.5, ("Skyrim.esm", 0x15B13), Some(("Skyrim.esm", 0x7265C)), m, GRID_FAR | LARGE | INITIALLY_ENABLED, (1, -1), ("Skyrim.esm", 0xB23FC), Some(("Skyrim.esm", 0x7265D)));
    f.add([5100.0, 20.0, 30.0], [0.0; 3], 1.0, ("Dawnguard.esm", 0x12), None, m, GRID_NEVER_FADE | (GLOW_LEVEL_NONE << GLOW_LEVEL_SHIFT), (1, 0), ("Skyrim.esm", 0x10), None);
    f.seasons.push(("WIN".into(), vec![(FormRef { plugin: 0, form: 0x15B13 }, m)]));
    f.resets.push(FormRef { plugin: 0, form: 0x33DCB });
    f.disabled.push(FormRef { plugin: 1, form: 0x12 });
    f.migration.push(FormRef { plugin: 0, form: 0x15B13 });
    f.child_worlds.push(ChildWorld { world: FormRef { plugin: 0, form: 0x1A26F }, flags: CHILD_USES_PARENT_LOD | CHILD_HAS_BOUNDS, bounds: [1, -4, 9, 3], editor_id: "WhiterunWorld".into() });
    f.underside = Some(Underside { model: r"meshes\terrain\tamriel\tamriel_underside.nif".into(), height: 500.0, worlds: vec![FormRef { plugin: 0, form: 0x3C }] });
    f.large.insert((1, -1), vec![LargeRef { reference: FormRef { plugin: 0, form: 0x15B13 }, lod: LRG_INST4 | LRG_DYN, bounds: [0.0, 1.0, 2.0, 3.0, 4.0, 5.0] }]);
    f.finish();
    for zstd in [None, Some(3)] {
        let bytes = f.to_bytes(WriteOptions { zstd, crc: true }).unwrap();
        let back = DynFile::from_bytes(&bytes).unwrap();
        assert_eq!(back.common, f.common);
        assert_eq!(back.header, f.header);
        assert_eq!(back.models, f.models);
        assert_eq!(back.seasons, f.seasons);
        assert_eq!((&back.resets, &back.disabled, &back.bases, &back.migration), (&f.resets, &f.disabled, &f.bases, &f.migration));
        assert_eq!(back.child_worlds, f.child_worlds);
        assert_eq!(back.underside, f.underside);
        assert_eq!(back.large, f.large);
        assert_eq!(back.cells, f.cells);
    }
    assert_eq!(f.items(), 2);
    let it = &f.cells[&(1, -1)][0];
    assert_eq!(it.hash, crate::hash::ref_hash("Skyrim.esm", 0x15B13));
    assert_eq!(it.grid(), GRID_FAR);
    assert!(it.has_parent());
    assert!(!f.cells[&(1, 0)][0].has_parent());
}

#[test]
fn dyn_direct_parent_offsets() {
    use crate::dynlod::*;
    let mut f = DynFile::new(common("W", ""));
    f.add([0.0; 3], [0.0; 3], 1.0, ("A.esm", 1), Some(("A.esm", 2)), 0, 0, (0, 0), ("A.esm", 3), Some(("B.esm", 4)));
    let bytes = f.to_bytes(WriteOptions { zstd: None, crc: true }).unwrap();
    let c = crate::container::Container::parse(&bytes, crate::container::Kind::Dyn).unwrap();
    let raw = c.payload(&c.index[0]).unwrap();
    assert_eq!(raw.len(), 64);
    assert_eq!(u16::from_le_bytes([raw[54], raw[55]]), 1); // B.esm
    assert_eq!(u32::from_le_bytes(raw[60..64].try_into().unwrap()), 4);
}
