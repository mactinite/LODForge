//! Billboard-cloud crowns: the leaves of a tree replaced by a few textured planes, the trunk kept as
//! reduced 3D geometry (docs/ultra-tree-models-plan.md, "Crown alternatives"). LODForge's own
//! method, after Décoret et al., "Billboard Clouds for Extreme Model Simplification" (SIGGRAPH
//! 2003); not part of TreeLODGen.
//!
//! 1. **Orientations.** Each leaf triangle (model space, transforms baked) goes to the vertical
//!    orientation ([`CloudParams::yaws`]) its card faces most, or to the horizontal one when the
//!    card is nearly flat ([`CloudParams::horizontal_min`]).
//!    A projected leaf then shows and hides with the view angle like the card it stands for.
//! 2. **Slices.** Per orientation the leaves are split by their distance along the normal into
//!    [`CloudParams::vertical_slices`] (or horizontal) equal slabs; each non-empty slab is one
//!    plane through the mean distance of its leaves, so no leaf moves far when projected.
//! 3. **Tiles.** [`plan`] returns per plane a small NIF of its leaves in the plane's frame, ready
//!    for TexGen's billboard camera at yaw 0 (plane right -> view right = -X, plane up -> +Z, plane
//!    normal -> +Y toward the camera), with the original shaders and textures; the caller renders
//!    colour and normal map and packs the tiles into one texture.
//! 4. **Quads.** [`quads`] makes one double-sided quad per plane with the tile's atlas UVs.

use lodforge_nif::{AvObject, Block, Nif, Node, ObjectNet, TriShape, VertexDesc};

use crate::collect::SourceShape;
use crate::levels::LevelPart;
use crate::mesh::PartMesh;

#[derive(Debug, Clone)]
pub struct CloudParams {
    /// Yaw angles (degrees) of the vertical plane normals; a plane serves both faces.
    pub yaws: Vec<f32>,
    pub vertical_slices: usize,
    pub horizontal_slices: usize,
    /// Tile sizes are relative to the tree: the largest tile gets `max_tile` pixels on its long
    /// side and the others the same pixels per unit (at least `min_tile`). Many tree models are
    /// small and scaled up where they are placed, so a fixed units-per-pixel left them with
    /// 50-pixel tiles (blotchy, straight-edged crowns in game).
    pub min_tile: u32,
    pub max_tile: u32,
    /// Smallest |normal . up| of a leaf triangle that goes to a horizontal slice (cos of its tilt
    /// from flat; 0.85 = within about 32 degrees).
    pub horizontal_min: f32,
    /// Render the full bark (branches, roots, twigs) into the slices as well as the leaves.
    pub bark_in_slices: bool,
    /// Turn the rendered soft coverage into on/off alpha by error diffusion ([dither_alpha]):
    /// the game alpha-tests the crown, which cuts away foliage thinner than a texel.
    pub dither: bool,
}

impl Default for CloudParams {
    fn default() -> Self {
        CloudParams { yaws: vec![0.0, 45.0, 90.0, 135.0], vertical_slices: 3, horizontal_slices: 3, min_tile: 16, max_tile: 256, horizontal_min: 0.85, bark_in_slices: true, dither: true }
    }
}

/// One plane of the cloud.
#[derive(Debug, Clone)]
pub struct Tile {
    /// Plane frame: right, up and normal (`u x v = n`), the plane at distance `d` along `n`.
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub n: [f32; 3],
    pub d: f32,
    /// Extent in plane coordinates (along `u` and `v`).
    pub a0: f32,
    pub a1: f32,
    pub b0: f32,
    pub b1: f32,
    /// Texture size of the tile.
    pub width: u32,
    pub height: u32,
    /// The plane's leaves in its frame (see the module docs), for rendering.
    pub nif: Nif,
    pub triangles: usize,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l > 1e-12 { v.map(|x| x / l) } else { v }
}

/// A plane frame: right, up, normal.
type Frame = ([f32; 3], [f32; 3], [f32; 3]);

/// Model-space positions and normals of a part.
type Baked = (Vec<[f32; 3]>, Vec<[f32; 3]>);

/// Orientation frames: (u, v, n) per vertical yaw, then the horizontal one (looking down).
fn frames(p: &CloudParams) -> Vec<Frame> {
    let mut f: Vec<_> = p
        .yaws
        .iter()
        .map(|y| {
            let (s, c) = y.to_radians().sin_cos();
            let n = [c, s, 0.0];
            let v = [0.0, 0.0, 1.0];
            (cross(v, n), v, n)
        })
        .collect();
    f.push(([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]));
    f
}

struct Leaf {
    part: usize,
    tri: usize,
    dist: f32,
}

/// Plans the planes of a crown from its level parts (shape-local meshes with their transforms).
pub fn plan(src: &Nif, shapes: &[SourceShape], crown: &[&LevelPart], p: &CloudParams) -> Vec<Tile> {
    let fr = frames(p);
    // Model-space positions and normals per part.
    let baked: Vec<Baked> = crown
        .iter()
        .map(|lp| {
            let pos: Vec<[f32; 3]> = lp.mesh.positions.iter().map(|q| lp.xf.point(*q)).collect();
            let nrm: Vec<[f32; 3]> = lp.mesh.normals.iter().map(|q| normalize(lp.xf.rot(*q))).collect();
            (pos, nrm)
        })
        .collect();
    let mut per: Vec<Vec<Leaf>> = (0..fr.len()).map(|_| Vec::new()).collect();
    for (pi, lp) in crown.iter().enumerate() {
        let pos = &baked[pi].0;
        for (ti, t) in lp.mesh.triangles.iter().enumerate() {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            let fnormal = normalize(cross([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]));
            // Horizontal slices only take nearly flat cards: LOD trees are seen from the side,
            // where a horizontal quad is edge-on, while a tilted real card still shows (an alder
            // sapling lost half its crown to the horizontal slices). Everything else goes to the
            // vertical orientation it faces most.
            let horizontal = fr.len() - 1;
            let o = if fnormal[2].abs() >= p.horizontal_min {
                horizontal
            } else {
                (0..horizontal).max_by(|&i, &j| dot(fnormal, fr[i].2).abs().partial_cmp(&dot(fnormal, fr[j].2).abs()).unwrap()).unwrap_or(horizontal)
            };
            let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
            if lp.kind == crate::split::Kind::Trunk {
                // Bark (a cylinder): assigned to one orientation by facing, each slice got only
                // the strip of a branch that faces it and branches broke into dotted lines.
                // The two vertical orientations it faces most; each slice then holds the faces
                // within 45 degrees, so a branch reads whole without four ghost copies.
                let mut by: Vec<usize> = (0..horizontal).collect();
                by.sort_by(|&i, &j| dot(fnormal, fr[j].2).abs().partial_cmp(&dot(fnormal, fr[i].2).abs()).unwrap());
                for &vo in by.iter().take(2) {
                    per[vo].push(Leaf { part: pi, tri: ti, dist: dot(centroid, fr[vo].2) });
                }
                continue;
            }
            per[o].push(Leaf { part: pi, tri: ti, dist: dot(centroid, fr[o].2) });
        }
    }
    let mut tiles = Vec::new();
    for (o, leaves) in per.iter().enumerate() {
        if leaves.is_empty() {
            continue;
        }
        let (u, v, n) = fr[o];
        let slices = if o + 1 == fr.len() { p.horizontal_slices } else { p.vertical_slices }.max(1);
        let lo = leaves.iter().map(|l| l.dist).fold(f32::INFINITY, f32::min);
        let hi = leaves.iter().map(|l| l.dist).fold(f32::NEG_INFINITY, f32::max);
        let width = ((hi - lo) / slices as f32).max(1e-3);
        for s in 0..slices {
            let slab: Vec<&Leaf> = leaves.iter().filter(|l| (((l.dist - lo) / width) as usize).min(slices - 1) == s).collect();
            if slab.is_empty() {
                continue;
            }
            let d = slab.iter().map(|l| l.dist).sum::<f32>() / slab.len() as f32;
            if let Some(t) = tile(src, shapes, crown, &baked, &slab, (u, v, n, d), p) {
                tiles.push(t);
            }
        }
    }
    // The largest tile side gets `max_tile` pixels; every tile the same pixels per unit.
    let largest = tiles.iter().map(|t| (t.a1 - t.a0).max(t.b1 - t.b0)).fold(0.0f32, f32::max).max(1e-3);
    let ppu = p.max_tile as f32 / largest;
    for t in &mut tiles {
        t.width = (((t.a1 - t.a0) * ppu).round() as u32).clamp(p.min_tile, p.max_tile);
        t.height = (((t.b1 - t.b0) * ppu).round() as u32).clamp(p.min_tile, p.max_tile);
    }
    tiles
}

fn tile(src: &Nif, shapes: &[SourceShape], crown: &[&LevelPart], baked: &[Baked], slab: &[&Leaf], (u, v, n, d): ([f32; 3], [f32; 3], [f32; 3], f32), p: &CloudParams) -> Option<Tile> {
    // Plane frame -> render frame: X = -a, Y = c, Z = b.
    let to_frame = |q: [f32; 3]| [-dot(q, u), dot(q, n) - d, dot(q, v)];
    let dir = |q: [f32; 3]| [-dot(q, u), dot(q, n), dot(q, v)];
    let mut out = Nif::new();
    out.header.block_types = vec!["BSFadeNode".into()];
    out.header.block_type_index = vec![0];
    out.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("Scene Root"), ..Default::default() }, ..Default::default() }));
    let (mut a0, mut a1, mut b0, mut b1) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    let mut children = Vec::new();
    let mut count = 0;
    for (pi, lp) in crown.iter().enumerate() {
        let tris: Vec<[u32; 3]> = slab.iter().filter(|l| l.part == pi).map(|l| lp.mesh.triangles[l.tri]).collect();
        if tris.is_empty() {
            continue;
        }
        let Some(Block::TriShape(s)) = src.blocks.get(shapes[lp.shape].block) else { continue };
        let (pos, nrm) = &baked[pi];
        let mut m = PartMesh { positions: pos.clone(), uvs: lp.mesh.uvs.clone(), normals: nrm.clone(), colors: lp.mesh.colors.clone(), triangles: tris, ..Default::default() };
        m.delete_unreferenced();
        for q in &m.positions {
            let (a, b) = (dot(*q, u), dot(*q, v));
            a0 = a0.min(a);
            a1 = a1.max(a);
            b0 = b0.min(b);
            b1 = b1.max(b);
        }
        count += m.triangles.len();
        let mut shape = TriShape { av: AvObject { net: ObjectNet::named("leaves"), ..Default::default() }, ..Default::default() };
        let (si, ti, ai) = clone_material(&mut out, src, s);
        shape.shader_property = si;
        shape.alpha_property = ai;
        let _ = ti;
        let has = [!m.uvs.is_empty(), m.normals.len() == m.positions.len(), m.colors.len() == m.positions.len()];
        shape.vertex_desc = VertexDesc::with_attributes(true, has[0], has[1], false, has[2]).0;
        shape.positions = m.positions.iter().map(|q| to_frame(*q)).collect();
        shape.normals = if has[1] { m.normals.iter().map(|q| dir(*q)).collect() } else { Vec::new() };
        shape.uvs = m.uvs.clone();
        shape.colors = if has[2] { m.colors.clone() } else { Vec::new() };
        shape.triangles = m.triangles.iter().map(|t| t.map(|i| i as u16)).collect();
        children.push(Some(out.add_block(Block::TriShape(shape))));
    }
    if children.is_empty() || !(a1 > a0 && b1 > b0) {
        return None;
    }
    if let Block::Node(r) = &mut out.blocks[0] {
        r.children = children;
    }
    // A small margin so edge texels are not cut.
    let (ma, mb) = ((a1 - a0) * 0.01, (b1 - b0) * 0.01);
    let (a0, a1, b0, b1) = (a0 - ma, a1 + ma, b0 - mb, b1 + mb);
    let _ = p;
    // Sizes are set by [plan] once every tile's extent is known.
    Some(Tile { u, v, n, d, a0, a1, b0, b1, width: 0, height: 0, nif: out, triangles: count })
}

/// The source shape's shader, texture set and alpha property copied into `out`.
fn clone_material(out: &mut Nif, src: &Nif, s: &TriShape) -> (Option<u32>, Option<u32>, Option<u32>) {
    let (mut si, mut ti) = (None, None);
    if let Some(Block::LightingShader(sh)) = s.shader_property.and_then(|r| src.blocks.get(r as usize)) {
        let textures = match sh.texture_set.and_then(|r| src.blocks.get(r as usize)) {
            Some(Block::TextureSet(t)) => t.clone(),
            _ => Vec::new(),
        };
        let t = out.add_block(Block::TextureSet(textures));
        let mut shader = sh.clone();
        shader.net = ObjectNet { name: shader.net.name.clone(), ..Default::default() };
        shader.texture_set = Some(t);
        si = Some(out.add_block(Block::LightingShader(shader)));
        ti = Some(t);
    }
    let ai = match s.alpha_property.and_then(|r| src.blocks.get(r as usize)) {
        Some(Block::Alpha { flags, threshold, .. }) => Some(out.add_block(Block::Alpha { net: ObjectNet::default(), flags: *flags, threshold: *threshold })),
        _ => None,
    };
    (si, ti, ai)
}

/// One double-sided quad per tile; `place[i]` = (x, y) of tile i in an atlas of `atlas` pixels.
/// Positions in model space, UVs into the atlas (V down), normal = plane normal; Skyrim's tangent
/// runs along the texture's V (plane down) and its bitangent along U (plane right), as the game's
/// normal maps (and lodforge-texgen's shader) read them.
pub fn quads(tiles: &[Tile], place: &[(u32, u32)], atlas: (u32, u32)) -> PartMesh {
    let mut m = PartMesh::default();
    let (aw, ah) = (atlas.0 as f32, atlas.1 as f32);
    for (t, &(x, y)) in tiles.iter().zip(place) {
        let base = m.positions.len() as u32;
        let at = |a: f32, b: f32| -> [f32; 3] { [t.n[0] * t.d + t.u[0] * a + t.v[0] * b, t.n[1] * t.d + t.u[1] * a + t.v[1] * b, t.n[2] * t.d + t.u[2] * a + t.v[2] * b] };
        let (u0, u1) = (x as f32 / aw, (x + t.width) as f32 / aw);
        let (v0, v1) = (y as f32 / ah, (y + t.height) as f32 / ah);
        // (a0,b0) bottom-left .. (a0,b1) top-left; texture row 0 = top.
        for (p, uv) in [(at(t.a0, t.b0), [u0, v1]), (at(t.a1, t.b0), [u1, v1]), (at(t.a1, t.b1), [u1, v0]), (at(t.a0, t.b1), [u0, v0])] {
            m.positions.push(p);
            m.uvs.push(uv);
            m.normals.push(t.n);
            m.tangents.push(t.v.map(|c| -c));
            m.bitangents.push(t.u);
            // White, as DynDOLOD's billboard and passthru shapes carry (Vertex_Colors).
            m.colors.push([255, 255, 255, 255]);
        }
        m.triangles.push([base, base + 1, base + 2]);
        m.triangles.push([base, base + 2, base + 3]);
    }
    m
}

/// Floyd-Steinberg error diffusion of the alpha channel (serpentine) into 0 / 255: a region with
/// coverage c keeps about c of its texels, spread out, so thin foliage survives the alpha test as
/// a fine pattern instead of vanishing below the threshold while dense clusters stay solid.
pub fn dither_alpha(rgba: &mut [u8], width: u32) {
    let w = width as usize;
    if w == 0 {
        return;
    }
    let h = rgba.len() / 4 / w;
    let mut err = vec![0.0f32; w * h];
    for y in 0..h {
        let ltr = y % 2 == 0;
        for i in 0..w {
            let x = if ltr { i } else { w - 1 - i };
            let k = y * w + x;
            let v = f32::from(rgba[k * 4 + 3]) / 255.0 + err[k];
            let out = if v >= 0.5 { 1.0 } else { 0.0 };
            rgba[k * 4 + 3] = if out > 0.0 { 255 } else { 0 };
            let e = v - out;
            let fwd = |dx: isize| if ltr { x as isize + dx } else { x as isize - dx };
            let mut add = |xx: isize, yy: usize, f: f32| {
                if xx >= 0 && (xx as usize) < w && yy < h {
                    err[yy * w + xx as usize] += e * f;
                }
            };
            add(fwd(1), y, 7.0 / 16.0);
            add(fwd(-1), y + 1, 3.0 / 16.0);
            add(x as isize, y + 1, 5.0 / 16.0);
            add(fwd(1), y + 1, 1.0 / 16.0);
        }
    }
}

/// Shelf packing of tile sizes into a power-of-two atlas: positions and the atlas size.
pub fn pack(sizes: &[(u32, u32)]) -> (Vec<(u32, u32)>, (u32, u32)) {
    let area: u64 = sizes.iter().map(|&(w, h)| u64::from(w) * u64::from(h)).sum();
    let widest = sizes.iter().map(|s| s.0).max().unwrap_or(1);
    let mut aw = ((area as f64 * 1.15).sqrt() as u32).max(widest).next_power_of_two();
    loop {
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(sizes[i].1));
        let mut pos = vec![(0, 0); sizes.len()];
        let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
        for i in order {
            let (w, h) = sizes[i];
            if x + w > aw {
                x = 0;
                y += shelf;
                shelf = 0;
            }
            pos[i] = (x, y);
            x += w;
            shelf = shelf.max(h);
        }
        let ah = (y + shelf).max(1).next_power_of_two();
        if ah <= aw * 2 {
            return (pos, (aw, ah));
        }
        aw *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dithering_keeps_the_coverage() {
        let (w, h) = (64u32, 64u32);
        let mut img = vec![0u8; (w * h * 4) as usize];
        for px in img.chunks_exact_mut(4) {
            px[3] = 77; // 30%
        }
        dither_alpha(&mut img, w);
        let on = img.chunks_exact(4).filter(|p| p[3] == 255).count() as f32 / (w * h) as f32;
        assert!((on - 77.0 / 255.0).abs() < 0.02, "{on}");
        assert!(img.chunks_exact(4).all(|p| p[3] == 0 || p[3] == 255));
    }

    #[test]
    fn frames_are_right_handed() {
        for (u, v, n) in frames(&CloudParams::default()) {
            let c = cross(u, v);
            assert!((c[0] - n[0]).abs() < 1e-5 && (c[1] - n[1]).abs() < 1e-5 && (c[2] - n[2]).abs() < 1e-5);
        }
    }

    #[test]
    fn packing_fits_every_tile() {
        let sizes = [(256, 200), (100, 50), (64, 64), (128, 256), (16, 16)];
        let (pos, (w, h)) = pack(&sizes);
        assert!(w.is_power_of_two() && h.is_power_of_two());
        for (i, &(x, y)) in pos.iter().enumerate() {
            assert!(x + sizes[i].0 <= w && y + sizes[i].1 <= h);
            for (j, &(x2, y2)) in pos.iter().enumerate().skip(i + 1) {
                let apart = x + sizes[i].0 <= x2 || x2 + sizes[j].0 <= x || y + sizes[i].1 <= y2 || y2 + sizes[j].1 <= y;
                assert!(apart, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn quad_faces_its_normal() {
        let t = Tile { u: [0.0, 1.0, 0.0], v: [0.0, 0.0, 1.0], n: [1.0, 0.0, 0.0], d: 5.0, a0: -1.0, a1: 1.0, b0: 0.0, b1: 2.0, width: 32, height: 32, nif: Nif::new(), triangles: 1 };
        let m = quads(&[t], &[(0, 0)], (32, 32));
        let [a, b, c] = m.triangles[0].map(|i| m.positions[i as usize]);
        let n = cross([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        assert!(n[0] > 0.0);
        assert_eq!(m.positions[0], [5.0, -1.0, 0.0]);
        assert_eq!(m.uvs[0], [0.0, 1.0]);
    }
}
