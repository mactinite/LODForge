//! Writes the synthetic `.dyn.bin` sample the LODForge Runtime reader test runs against
//! (LODForge-Runtime `tests/data`): `Fixture.dyn.bin` and `Build.txt`, with made-up plugins and
//! models and every item kind the reader checks (near / far grid, animated, glow, enable parent
//! with opposite, replace + `DDIS`, child copy, a negative cell, a section large enough for zstd).
//! Deterministic: rerun it after a format change and commit the result there.
//!
//! `cargo run -p lodforge-instances --example dyn_fixture -- <out dir>`
//! Expected counts for the test: 45 items, 3 models, 1 animated item.

use lodforge_instances::container::{Common, WriteOptions, MAJOR, MINOR};
use lodforge_instances::dynlod::*;

const TOKEN: &str = "fixture";
const GENERATOR: &str = "LODForge dyn_fixture";

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: dyn_fixture <out dir>"));
    let mut f = DynFile::new(Common { world: "Fixture".into(), generator: GENERATOR.into(), token: TOKEN.into(), ..Default::default() });
    f.header = DynHeader { world: f.form("Fixture.esm", 0x3C), south_west: (-8, -8), near_grid: 5, far_grid: 11, glow_multiple: 1.0, lod_world: String::new(), flags: 0 };

    let rock = f.model("rock", || DynModel { flags: MODEL_FULL, pack: [0, NO_MESH, NO_MESH, NO_MESH], bounds: [-64.0, -64.0, 0.0, 64.0, 64.0, 96.0], model: r"meshes\fixture\rock01.nif".into(), ..Default::default() });
    let mill = f.model("mill", || DynModel { flags: MODEL_FULL | MODEL_ANIMATED, pack: [NO_MESH; 4], bounds: [-200.0, -200.0, 0.0, 200.0, 200.0, 900.0], model: r"meshes\fixture\windmill.nif".into(), ..Default::default() });
    let lamp = f.model("lamp", || DynModel { flags: MODEL_FULL | MODEL_GLOW, pack: [NO_MESH; 4], bounds: [-16.0, -16.0, 0.0, 16.0, 16.0, 128.0], model: r"meshes\fixture\lamp.nif".into(), ..Default::default() });

    let esp = "Fixture.esp";
    let base = ("Fixture.esm", 0x900);
    // Cell (0, 0): forty near-grid rocks, enough repeated bytes for a compressed section.
    for i in 0..40u32 {
        let pos = [100.0 + 90.0 * i as f32, 200.0 + 50.0 * (i % 7) as f32, 10.0 * (i % 3) as f32];
        f.add(pos, [0.0, 0.0, 0.1 * i as f32], 1.0, (esp, 0x800 + i), None, rock, GRID_NEAR | INITIALLY_ENABLED, cell_of(pos), base, None);
    }
    // Cell (1, 0): an animated and a glowing item.
    let pos = [5000.0, 1000.0, 0.0];
    f.add(pos, [0.0; 3], 1.0, (esp, 0xA00), None, mill, GRID_FAR | INITIALLY_ENABLED | FULL | ANIMATED, cell_of(pos), ("Fixture.esm", 0x901), None);
    let pos = [6000.0, 1200.0, 0.0];
    f.add(pos, [0.0; 3], 1.0, (esp, 0xA01), None, lamp, GRID_NEAR | INITIALLY_ENABLED | FULL | GLOW, cell_of(pos), ("Fixture.esm", 0x902), None);
    // Cell (-1, -1): enable parent with opposite, a replaced original, a child copy.
    let pos = [-100.0, -100.0, 0.0];
    f.add(pos, [0.0; 3], 1.5, (esp, 0xB00), Some(("Fixture.esm", 0x100)), rock, GRID_FAR | OPPOSITE, cell_of(pos), base, None);
    let pos = [-300.0, -200.0, 0.0];
    f.add(pos, [0.0; 3], 1.0, (esp, 0xB01), None, rock, GRID_NEVER_FADE | INITIALLY_ENABLED | REPLACE, cell_of(pos), base, None);
    let replaced = f.form(esp, 0xB01);
    f.disabled.push(replaced);
    let pos = [-500.0, -400.0, 0.0];
    f.add(pos, [0.0; 3], 1.0, (esp, 0xB02), None, rock, GRID_NEAR | INITIALLY_ENABLED | CHILD_COPY, cell_of(pos), base, None);
    f.finish();

    std::fs::create_dir_all(&out).unwrap();
    let bytes = f.to_bytes(WriteOptions { zstd: Some(3), crc: true }).unwrap();
    std::fs::write(out.join(format!("Fixture.{EXT}")), bytes).unwrap();
    std::fs::write(out.join("Build.txt"), format!("token={TOKEN}\nformat={MAJOR}.{MINOR}\ngenerator={GENERATOR}\nworlds=Fixture\n")).unwrap();
    println!("{}: {} items, {} models", out.display(), f.items(), f.models.len());
}
