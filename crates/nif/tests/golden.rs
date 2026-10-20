//! Byte-exact round trip of golden terrain BTRs and object BTOs: `write(parse(bytes)) == bytes`.
//! The reader and writer are both written from the NIF format specification; agreeing with real
//! files byte for byte shows the encoding is the same.
//!
//! Golden folder: `LODFORGE_GOLDEN_BTR` or the default below. Skipped (passes with a note) when
//! the folder does not exist.

use std::path::{Path, PathBuf};

use lodforge_nif::parse;

const DEFAULT_DIR: &str =
    r"D:\Modlists\LODForge-golden\no-seasons-20261017-031139\output\meshes\terrain\Tamriel";

fn golden_dir() -> PathBuf {
    std::env::var("LODFORGE_GOLDEN_BTR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIR))
}

fn files_with(dir: &Path, ext: &str) -> Option<Vec<PathBuf>> {
    if !dir.is_dir() {
        eprintln!("golden folder {} missing, skipping", dir.display());
        return None;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    files.sort();
    Some(files)
}

fn first_mismatch(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or((a.len() != b.len()).then_some(a.len().min(b.len())))
}

fn check<F: Fn(&[u8]) -> Result<Vec<u8>, String>>(files: &[PathBuf], what: &str, f: F) {
    let mut failures = Vec::new();
    for p in files {
        let bytes = std::fs::read(p).unwrap();
        match f(&bytes) {
            Ok(out) => {
                if let Some(off) = first_mismatch(&bytes, &out) {
                    failures.push(format!(
                        "{}: first mismatch at {off:#x} (golden {} bytes, written {})",
                        name(p),
                        bytes.len(),
                        out.len()
                    ));
                }
            }
            Err(e) => failures.push(format!("{}: {e}", name(p))),
        }
    }
    println!(
        "{what}: {}/{} byte-identical",
        files.len() - failures.len(),
        files.len()
    );
    for f in failures.iter().take(20) {
        println!("  {f}");
    }
    assert!(
        failures.is_empty(),
        "{what}: {} of {} files differ",
        failures.len(),
        files.len()
    );
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Parse + write with the parsed header tables kept.
#[test]
fn golden_btr_round_trip() {
    let Some(files) = files_with(&golden_dir(), "btr") else {
        return;
    };
    assert!(!files.is_empty());
    check(&files, "round trip", |bytes| {
        let nif = parse(bytes).map_err(|e| e.to_string())?;
        if !nif.raw_blocks.is_empty() {
            return Err(format!(
                "raw (non re-encoded) blocks {:?}",
                nif.raw_blocks.keys().collect::<Vec<_>>()
            ));
        }
        nif.to_bytes().map_err(|e| e.to_string())
    });
}

/// Parse, drop the header's block type and string tables, write: checks that the tables are
/// rebuilt in first-use order, which is the order the golden files use.
#[test]
fn golden_btr_rebuilt_tables() {
    let Some(files) = files_with(&golden_dir(), "btr") else {
        return;
    };
    check(&files, "rebuilt tables", |bytes| {
        let mut nif = parse(bytes).map_err(|e| e.to_string())?;
        nif.header.block_types.clear();
        nif.header.block_type_index.clear();
        nif.header.block_sizes.clear();
        nif.header.strings.clear();
        nif.to_bytes().map_err(|e| e.to_string())
    });
}

/// Object LOD (.bto) files of the same golden run (`<golden>\Objects`), incl. the
/// BSDistantObjectLargeRefExtraData blocks: parse + write with no raw blocks, byte-identical.
#[test]
fn golden_bto_round_trip() {
    let Some(files) = files_with(&golden_dir().join("Objects"), "bto") else {
        return;
    };
    assert!(!files.is_empty());
    check(&files, "bto round trip", |bytes| {
        let nif = parse(bytes).map_err(|e| e.to_string())?;
        if !nif.raw_blocks.is_empty() {
            return Err(format!(
                "raw (non re-encoded) blocks {:?}",
                nif.raw_blocks.keys().collect::<Vec<_>>()
            ));
        }
        nif.to_bytes().map_err(|e| e.to_string())
    });
}

/// Season variants of a terrain quad differ only in the texture paths: re-encoding the texture
/// set block alone (`writer::Encoded::replace_block`) gives the same bytes as a full write.
#[test]
fn golden_btr_texture_block_replacement() {
    let Some(files) = files_with(&golden_dir(), "btr") else {
        return;
    };
    let mut checked = 0;
    for p in files.iter().step_by(97) {
        let mut nif = parse(&std::fs::read(p).unwrap()).unwrap();
        let mut enc = lodforge_nif::writer::encode(&nif).unwrap();
        let Some(i) = nif
            .blocks
            .iter()
            .position(|b| matches!(b, lodforge_nif::Block::TextureSet(_)))
        else {
            continue;
        };
        let lodforge_nif::Block::TextureSet(paths) = &nif.blocks[i] else {
            unreachable!()
        };
        let paths: Vec<String> = paths
            .iter()
            .map(|t| t.replacen(".dds", ".WIN.dds", 1))
            .collect();
        nif.blocks[i] = lodforge_nif::Block::TextureSet(paths);
        enc.replace_block(&nif, i).unwrap();
        assert_eq!(
            enc.to_bytes(&nif.header, &nif.roots).unwrap(),
            nif.to_bytes().unwrap(),
            "{}",
            name(p)
        );
        checked += 1;
    }
    println!("texture block replacement: {checked} files");
    assert!(checked > 0);
}
