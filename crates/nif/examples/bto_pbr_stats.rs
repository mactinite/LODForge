//! PBR LOD4 BTO statistics of a folder (`<dir> [prefix]`): shapes whose diffuse names rock,
//! snow, ice, glacier or mountain; how many use PBR textures (`textures\pbr\`); vertex-weighted
//! mean RGB vertex colour; plus the same for all non-atlas shapes and all shapes.
use lodforge_nif::{parse, Block};

#[derive(Default)]
struct Acc {
    shapes: usize,
    pbr: usize,
    verts: f64,
    rgb: f64,
}

impl Acc {
    fn add(&mut self, pbr: bool, colors: &[[u8; 4]], n: usize) {
        self.shapes += 1;
        self.pbr += usize::from(pbr);
        if colors.is_empty() {
            self.verts += n as f64;
            self.rgb += 255.0 * n as f64;
        } else {
            self.verts += colors.len() as f64;
            self.rgb += colors.iter().map(|c| (f64::from(c[0]) + f64::from(c[1]) + f64::from(c[2])) / 3.0).sum::<f64>();
        }
    }
    fn line(&self, what: &str) -> String {
        format!("{what:<22} shapes {:6}  pbr {:6} ({:5.1}%)  mean vc {:6.1}", self.shapes, self.pbr, 100.0 * self.pbr as f64 / self.shapes.max(1) as f64, self.rgb / self.verts.max(1.0))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prefix = args.get(1).cloned().unwrap_or_else(|| "Tamriel.4.".into());
    let (mut rock, mut hd, mut all) = (Acc::default(), Acc::default(), Acc::default());
    let mut files = 0;
    for e in std::fs::read_dir(&args[0]).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) || !name.ends_with(".bto") || name.matches('.').count() != 4 {
            continue;
        }
        let Ok(nif) = parse(&std::fs::read(e.path()).unwrap()) else { continue };
        files += 1;
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let tex = nif.shape_textures(s);
            let d = tex.first().map(|t| t.to_ascii_lowercase()).unwrap_or_default();
            let pbr = d.contains("textures\\pbr\\") || d.starts_with("pbr\\");
            all.add(pbr, &s.colors, s.positions.len());
            if d.contains("dyndolod\\lod\\") {
                continue;
            }
            hd.add(pbr, &s.colors, s.positions.len());
            if ["rock", "snow", "ice", "glacier", "mountain"].iter().any(|k| d.rsplit('\\').next().unwrap_or("").contains(k)) {
                rock.add(pbr, &s.colors, s.positions.len());
                if !pbr && std::env::var_os("LODFORGE_NONPBR").is_some() {
                    println!("  non-pbr {d}");
                }
            }
        }
    }
    println!("{files} files");
    println!("{}", rock.line("rock/snow/ice (HD)"));
    println!("{}", hd.line("non-atlas"));
    println!("{}", all.line("all"));
}
