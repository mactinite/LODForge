//! Far grass for the LODForge Runtime: `SKSE\Plugins\LODForgeRuntime\<World>[.<SEASON>].lfg`
//! (docs/runtime/instance-formats.md section 7) from an NGIO grass cache. The cache is read as a
//! file format only (`lodforge_grass::cgid`); LODForge generates no grass.
//!
//! Also `lodforge runtime-grass`, which prints the file sizes per density level.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;

use lodforge_grass::cgid::{CellCache, TypeCache, strip_meshes};
use lodforge_instances::grass::{GrassCell, GrassFile, GrassType, LEVELS, UNRESOLVED, instances_from_cgid};
use lodforge_instances::{Common, WriteOptions};

use crate::instances::FormNames;
use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Extra seasons (SPR, SUM, AUT, WIN): `.lfg` files from the `.<season>.cgid` caches.
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Folder with the `.cgid` files (default: `Grass\` of the load order).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Output Data folder.
    #[arg(long)]
    pub out: PathBuf,
    /// zstd level for the cell sections (0 = uncompressed).
    #[arg(long, default_value_t = 0)]
    pub zstd: i32,
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let opts = WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true };
    let reports = build(&p, &a.world, &a.seasons, a.cache.as_deref(), &a.out, opts)?;
    for r in &reports {
        print!("{}", r.summary());
        // Read back: the reader parses every section and checks the CRCs.
        let back = GrassFile::from_bytes(&std::fs::read(&r.file)?)?;
        anyhow::ensure!(back.records() == r.records && back.cells.len() == r.cells, "{} reads back differently", r.file.display());
    }
    Ok(())
}

/// One written `.lfg`.
pub struct Report {
    pub file: PathBuf,
    pub bytes: usize,
    pub cells: usize,
    pub records: usize,
    pub types: usize,
    pub unresolved_types: usize,
    /// Per density level k (fraction 2^-k): uncompressed bytes of a file holding only those
    /// records, and with zstd (when it was on) the compressed size.
    pub level_raw: [u64; LEVELS],
    pub level_zstd: Option<[u64; LEVELS]>,
    pub source_bytes: u64,
    pub ms: u128,
}

impl Report {
    pub fn summary(&self) -> String {
        let mb = |b: u64| b as f64 / 1e6;
        let mut s = format!(
            "  {}: {} cells, {} instances, {} grass types ({} not in this load order); {:.1} MB written from {:.1} MB of .cgid in {:.1} s\n",
            self.file.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
            self.cells,
            self.records,
            self.types,
            self.unresolved_types,
            mb(self.bytes as u64),
            mb(self.source_bytes),
            self.ms as f64 / 1000.0
        );
        for k in 0..LEVELS {
            s.push_str(&format!("    level {k} (1/{:<3}): {:>8.1} MB raw", 1u32 << k, mb(self.level_raw[k])));
            if let Some(z) = &self.level_zstd {
                s.push_str(&format!(", {:>8.1} MB zstd", mb(z[k])));
            }
            s.push('\n');
        }
        s
    }
}

/// A cache file: cell and season infix (`""` for the default season, else lower case).
fn parse_name(name: &str, world_l: &str) -> Option<((i32, i32), String)> {
    let n = name.to_ascii_lowercase();
    let rest = n.strip_prefix(world_l)?.strip_prefix('x')?.strip_suffix(".cgid")?;
    let (x, rest) = rest.split_once('y')?;
    let (y, season) = match rest.split_once('.') {
        Some((y, s)) => (y, s.to_string()),
        None => (rest, String::new()),
    };
    Some(((x.parse().ok()?, y.parse().ok()?), season))
}

/// Where the cache files are: cell -> (season infix -> reader).
enum Source<'a> {
    Dir(&'a Path),
    Vfs(&'a Profile),
}

impl Source<'_> {
    fn list(&self, world: &str) -> Result<Vec<(String, (i32, i32), String)>> {
        let world_l = world.to_ascii_lowercase();
        let names: Vec<String> = match self {
            Source::Dir(d) => std::fs::read_dir(d).with_context(|| format!("listing {}", d.display()))?.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect(),
            Source::Vfs(p) => p.vfs.list_prefix(&format!("grass\\{world_l}x")).into_iter().filter_map(|r| r.strip_prefix("grass\\").map(str::to_string)).collect(),
        };
        Ok(names.into_iter().filter_map(|n| parse_name(&n, &world_l).map(|(c, s)| (n, c, s))).collect())
    }

    fn read(&self, name: &str) -> Option<Vec<u8>> {
        match self {
            Source::Dir(d) => std::fs::read(d.join(name)).ok(),
            Source::Vfs(p) => p.vfs.read(&format!("grass\\{name}")).ok().flatten(),
        }
    }
}

/// Builds and writes the `.lfg` of the default season and of every season in `seasons` that
/// has cache files (cells without a season cache use the default season's).
pub fn build(p: &Profile, world: &str, seasons: &[String], cache: Option<&Path>, out: &Path, opts: WriteOptions) -> Result<Vec<Report>> {
    let src = match cache {
        Some(d) => Source::Dir(d),
        None => Source::Vfs(p),
    };
    let files = src.list(world)?;
    if files.is_empty() {
        println!(
            "  far grass: no NGIO grass cache for {world} in {}; no .lfg written (docs/runtime/grass-lfg-sizes.md says how to precache)",
            cache.map(|d| d.display().to_string()).unwrap_or_else(|| "Grass\\ of the load order".into())
        );
        return Ok(Vec::new());
    }
    // Current GRAS records by (model, object id), for the cache's load-order form ids.
    let (gras, _) = crate::grass_types::grass_tables(p)?;
    let object_id = |id: u32| if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
    let current: HashMap<(String, u32), u32> = gras.iter().map(|g| ((strip_meshes(&g.model).to_ascii_lowercase(), object_id(g.form_id)), g.form_id)).collect();
    let names = FormNames::new(p);
    let mut by_cell: HashMap<(i32, i32), HashMap<String, String>> = HashMap::new();
    for (name, cell, season) in files {
        by_cell.entry(cell).or_default().insert(season, name);
    }
    let mut reports = Vec::new();
    let mut wanted: Vec<String> = vec![String::new()];
    wanted.extend(seasons.iter().map(|s| s.to_ascii_lowercase()));
    for season in wanted {
        if !season.is_empty() && !by_cell.values().any(|m| m.contains_key(&season)) {
            println!("  far grass: no .{season}.cgid caches for {world}; the runtime uses the default season's .lfg");
            continue;
        }
        let t = Instant::now();
        let mut cells: Vec<((i32, i32), &String)> = by_cell.iter().filter_map(|(c, m)| m.get(&season).or_else(|| m.get("")).map(|n| (*c, n))).collect();
        cells.sort();
        // Parse in parallel; types are collected per cell and numbered afterwards.
        let parsed: Vec<((i32, i32), u64, Option<CellCache>)> = cells
            .par_iter()
            .map(|(c, n)| match src.read(n) {
                Some(bytes) => (*c, bytes.len() as u64, CellCache::parse_prefix(&bytes).map(|(cc, _)| cc)),
                None => (*c, 0, None),
            })
            .collect();
        let source_bytes: u64 = parsed.iter().map(|(_, b, _)| *b).sum();
        let mut file = GrassFile { common: Common { world: world.to_string(), season: season.to_ascii_uppercase(), generator: crate::instances::generator(), plugins: Vec::new(), params: format!("source=ngio-cgid\nlevels={LEVELS}"), token: crate::instances::build_token() }, ..Default::default() };
        let mut type_index: HashMap<(String, u32, u16), u16> = HashMap::new();
        let mut unresolved = 0usize;
        let mut cell_types: Vec<Vec<u16>> = Vec::with_capacity(parsed.len());
        for (_, _, cc) in &parsed {
            let mut v = Vec::new();
            for t in cc.iter().flat_map(|c| &c.types) {
                let model = format!("meshes\\{}", strip_meshes(&t.model).to_ascii_lowercase());
                let key = (strip_meshes(&t.model).to_ascii_lowercase(), object_id(t.form_id));
                let (plugin, form) = match current.get(&key).and_then(|&id| names.split(id)) {
                    Some((n, local)) => (file.common.plugin_index(n), local),
                    None => (UNRESOLVED, t.form_id),
                };
                let flags = u16::from(t.fits_to_slope) | (u16::from(t.uniform_scaling) << 1) | (u16::from(t.vertex_lighting) << 2);
                let idx = *type_index.entry((model.clone(), form, plugin)).or_insert_with(|| {
                    if plugin == UNRESOLVED {
                        unresolved += 1;
                    }
                    file.types.push(GrassType { plugin, flags, form, wave_period: t.wave_period, model });
                    (file.types.len() - 1) as u16
                });
                v.push(idx);
            }
            cell_types.push(v);
        }
        let built: Vec<Result<Option<GrassCell>>> = parsed
            .par_iter()
            .zip(cell_types.par_iter())
            .map(|((c, _, cc), types)| -> Result<Option<GrassCell>> {
                let Some(cc) = cc else { return Ok(None) };
                // `instances_from_cgid` asks once per type, in file order.
                let mut k = 0usize;
                let inst = instances_from_cgid(*c, cc, |_: &TypeCache| {
                    k += 1;
                    types.get(k - 1).copied()
                });
                if inst.is_empty() {
                    return Ok(None);
                }
                let (x, y) = (i16::try_from(c.0)?, i16::try_from(c.1)?);
                Ok(Some(GrassCell::build(x, y, &inst)?))
            })
            .collect();
        for b in built {
            if let Some(cell) = b? {
                file.cells.insert((cell.x, cell.y), cell);
            }
        }
        let bytes = file.to_bytes(opts)?;
        let dir = crate::instances::out_dir(out);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(file.common.file_name(lodforge_instances::grass::EXT));
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        let (level_raw, level_zstd) = level_sizes(&file, opts.zstd);
        reports.push(Report { file: path, bytes: bytes.len(), cells: file.cells.len(), records: file.records(), types: file.types.len(), unresolved_types: unresolved, level_raw, level_zstd, source_bytes, ms: t.elapsed().as_millis() });
    }
    Ok(reports)
}

/// Sizes of a file that held only the first `prefix[k]` records of every cell.
fn level_sizes(f: &GrassFile, zstd: Option<i32>) -> ([u64; LEVELS], Option<[u64; LEVELS]>) {
    let fixed = 64 + 32 * f.cells.len() as u64 + 4096;
    let mut raw = [fixed; LEVELS];
    for c in f.cells.values() {
        for (k, r) in raw.iter_mut().enumerate() {
            *r += (c.header_size() + c.prefix[k] as usize * lodforge_instances::grass::RECORD) as u64;
        }
    }
    let z = zstd.map(|level| {
        let per: Vec<[u64; LEVELS]> = f
            .cells
            .par_iter()
            .map(|(_, c)| {
                let full = c.to_payload();
                let mut v = [0u64; LEVELS];
                for (k, out) in v.iter_mut().enumerate() {
                    let n = c.header_size() + c.prefix[k] as usize * lodforge_instances::grass::RECORD;
                    *out = zstd::bulk::compress(&full[..n], level).map(|z| z.len().min(n) as u64).unwrap_or(n as u64);
                }
                v
            })
            .collect();
        let mut t = [fixed; LEVELS];
        for v in per {
            for k in 0..LEVELS {
                t[k] += v[k];
            }
        }
        t
    });
    (raw, z)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cache_names() {
        assert_eq!(super::parse_name("tamrielx-047y0035.cgid", "tamriel"), Some(((-47, 35), String::new())));
        assert_eq!(super::parse_name("Tamrielx0000y-010.AUT.cgid", "tamriel"), Some(((0, -10), "aut".into())));
        assert_eq!(super::parse_name("tamrielx0000y0000.gid", "tamriel"), None);
        assert_eq!(super::parse_name("blackreachx0000y0000.cgid", "tamriel"), None);
    }
}
