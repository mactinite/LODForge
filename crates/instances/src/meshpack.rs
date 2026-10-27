//! `TreeMeshes.bin` (docs/runtime/instance-formats.md section 7a): the near-tier tree models of a
//! build merged into one vertex buffer, one index buffer and a table of fixed-size clusters, with
//! their textures on shared atlas pages, so the runtime draws every visible (instance, cluster)
//! pair of a page with one indirect draw.

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::{Error, Result};

pub const FILE_NAME: &str = "TreeMeshes.bin";
/// Triangles per cluster (the last cluster of a run is padded with degenerate triangles).
pub const CLUSTER_TRIANGLES: usize = 64;
pub const VERTEX_SIZE: usize = 28;
pub const CLUSTER_SIZE: usize = 40;
/// Section ids (the index entry's `level`).
pub const SECTION_VERTICES: u8 = 1;
pub const SECTION_INDICES: u8 = 2;
pub const SECTION_CLUSTERS: u8 = 3;
/// `TMPK` value of a type without a pack mesh.
pub const NO_MESH: u32 = u32::MAX;
/// Page classes (`Page::class`, statics `MCLS`): the LOD material the runtime gives the page's
/// proxy (shape name `obj`, `objSnow`, `objSnowHD`, `objAsh`, `objAshHD`).
pub const CLASS_PLAIN: u8 = 0;
pub const CLASS_SNOW: u8 = 1;
pub const CLASS_SNOW_HD: u8 = 2;
pub const CLASS_ASH: u8 = 3;
pub const CLASS_ASH_HD: u8 = 4;
/// Far grass: Community Shaders "complex grass" page, twice the atlas height: colour tiles in the
/// top half, the same tiles' normal (RGB) and specular (A) in the bottom half, so CS's complex
/// grass sampling (colour at v * 0.5, normal at 0.5 + v * 0.5) works on the page. Mesh UVs are in
/// the top half's space (v = 1 is its bottom).
pub const CLASS_GRASS_COMPLEX: u8 = 5;

/// One vertex as stored (28 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Model space.
    pub pos: [f32; 3],
    /// 10:10:10:2 unorm of `n * 0.5 + 0.5` (w unused, 0).
    pub normal: u32,
    /// 10:10:10:2 unorm of `t * 0.5 + 0.5`; w = 1 when the bitangent is `cross(n, t)`, 0 when
    /// it is `-cross(n, t)`.
    pub tangent: u32,
    /// Page UV, unorm16 (`u = v16 / 65535`).
    pub uv: [u16; 2],
    pub color: [u8; 4],
}

pub fn pack_unit(v: [f32; 3], w: u32) -> u32 {
    let q = |x: f32| ((x.clamp(-1.0, 1.0) * 0.5 + 0.5) * 1023.0).round() as u32;
    q(v[0]) | (q(v[1]) << 10) | (q(v[2]) << 20) | ((w & 3) << 30)
}

pub fn unpack_unit(p: u32) -> ([f32; 3], u32) {
    let f = |s: u32| ((p >> s) & 1023) as f32 / 1023.0 * 2.0 - 1.0;
    ([f(0), f(10), f(20)], p >> 30)
}

/// One cluster (40 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cluster {
    /// First of its `3 * CLUSTER_TRIANGLES` indices in the index buffer.
    pub first_index: u32,
    /// The mesh's first vertex (indices are absolute; this is for the runtime's convenience).
    pub first_vertex: u32,
    pub mesh: u32,
    pub page: u16,
    /// Real triangles (the rest of the cluster is degenerate).
    pub triangles: u8,
    pub flags: u8,
    /// Model-space AABB of its triangles.
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One atlas page (`MPPG`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// Alpha test threshold (0..255, `alpha >= threshold` is drawn).
    pub alpha_threshold: u8,
    /// LOD material class of every cluster on the page (`CLASS_*`, since 1.5).
    pub class: u8,
    pub albedo: String,
    pub normal: String,
}

/// One merged model (`MPMS`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub first_cluster: u32,
    pub cluster_count: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
    /// Real triangles (without the padding).
    pub triangles: u32,
    pub bounds: [f32; 6],
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshPack {
    pub common: Common,
    pub pages: Vec<Page>,
    pub meshes: Vec<Mesh>,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub clusters: Vec<Cluster>,
}

impl MeshPack {
    pub fn mesh_index(&self, model: &str) -> Option<u32> {
        self.meshes.iter().position(|m| m.model.eq_ignore_ascii_case(model)).map(|i| i as u32)
    }

    /// GPU bytes of the three buffers.
    pub fn buffer_bytes(&self) -> usize {
        self.vertices.len() * VERTEX_SIZE + self.indices.len() * 4 + self.clusters.len() * CLUSTER_SIZE
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        let mut b = Buf::default();
        b.u32(self.pages.len() as u32);
        for p in &self.pages {
            b.u32(p.width);
            b.u32(p.height);
            b.u16(p.mips as u16);
            b.u8(p.alpha_threshold);
            b.u8(p.class);
            b.str(&p.albedo);
            b.str(&p.normal);
        }
        chunks.push(Chunk { tag: *b"MPPG", data: b.0 });
        let mut b = Buf::default();
        b.u32(self.meshes.len() as u32);
        for m in &self.meshes {
            b.u32(m.first_cluster);
            b.u32(m.cluster_count);
            b.u32(m.first_vertex);
            b.u32(m.vertex_count);
            b.u32(m.triangles);
            for v in m.bounds {
                b.f32(v);
            }
            b.str(&m.model);
        }
        chunks.push(Chunk { tag: *b"MPMS", data: b.0 });
        let mut vb = Buf(Vec::with_capacity(self.vertices.len() * VERTEX_SIZE));
        for v in &self.vertices {
            for c in v.pos {
                vb.f32(c);
            }
            vb.u32(v.normal);
            vb.u32(v.tangent);
            vb.u16(v.uv[0]);
            vb.u16(v.uv[1]);
            vb.0.extend(v.color);
        }
        let mut ib = Buf(Vec::with_capacity(self.indices.len() * 4));
        for &i in &self.indices {
            ib.u32(i);
        }
        let mut cb = Buf(Vec::with_capacity(self.clusters.len() * CLUSTER_SIZE));
        for c in &self.clusters {
            cb.u32(c.first_index);
            cb.u32(c.first_vertex);
            cb.u32(c.mesh);
            cb.u16(c.page);
            cb.u8(c.triangles);
            cb.u8(c.flags);
            for v in c.min.iter().chain(&c.max) {
                cb.f32(*v);
            }
        }
        let sections = vec![
            RawSection { level: SECTION_VERTICES, x: 0, y: 0, data: vb.0, records: self.vertices.len() as u32 },
            RawSection { level: SECTION_INDICES, x: 0, y: 0, data: ib.0, records: self.indices.len() as u32 },
            RawSection { level: SECTION_CLUSTERS, x: 0, y: 0, data: cb.0, records: self.clusters.len() as u32 },
        ];
        write(Kind::MeshPack, &chunks, sections, opts)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<MeshPack> {
        let c = Container::parse(bytes, Kind::MeshPack)?;
        c.verify()?;
        let mut pack = MeshPack { common: c.common()?, ..Default::default() };
        if let Some(ch) = c.chunk(b"MPPG") {
            let mut r = Cur::new(&ch.data);
            for _ in 0..r.u32()? {
                let (width, height, mips, alpha_threshold) = (r.u32()?, r.u32()?, u32::from(r.u16()?), r.u8()?);
                let class = r.u8()?;
                pack.pages.push(Page { width, height, mips, alpha_threshold, class, albedo: r.str()?, normal: r.str()? });
            }
        }
        if let Some(ch) = c.chunk(b"MPMS") {
            let mut r = Cur::new(&ch.data);
            for _ in 0..r.u32()? {
                let (first_cluster, cluster_count, first_vertex, vertex_count, triangles) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
                let mut bounds = [0f32; 6];
                for v in &mut bounds {
                    *v = r.f32()?;
                }
                pack.meshes.push(Mesh { first_cluster, cluster_count, first_vertex, vertex_count, triangles, bounds, model: r.str()? });
            }
        }
        for e in &c.index {
            let raw = c.payload(e)?;
            let mut r = Cur::new(&raw);
            let n = e.record_count as usize;
            match e.level {
                SECTION_VERTICES => {
                    for _ in 0..n {
                        let pos = [r.f32()?, r.f32()?, r.f32()?];
                        let (normal, tangent) = (r.u32()?, r.u32()?);
                        let uv = [r.u16()?, r.u16()?];
                        let color = r.take(4)?.try_into().unwrap();
                        pack.vertices.push(Vertex { pos, normal, tangent, uv, color });
                    }
                }
                SECTION_INDICES => {
                    for _ in 0..n {
                        pack.indices.push(r.u32()?);
                    }
                }
                SECTION_CLUSTERS => {
                    for _ in 0..n {
                        let (first_index, first_vertex, mesh, page, triangles, flags) = (r.u32()?, r.u32()?, r.u32()?, r.u16()?, r.u8()?, r.u8()?);
                        let min = [r.f32()?, r.f32()?, r.f32()?];
                        let max = [r.f32()?, r.f32()?, r.f32()?];
                        pack.clusters.push(Cluster { first_index, first_vertex, mesh, page, triangles, flags, min, max });
                    }
                }
                _ => {}
            }
        }
        if pack.indices.len() != pack.clusters.len() * 3 * CLUSTER_TRIANGLES {
            return Err(Error::Invalid("index count is not 3 * 64 per cluster"));
        }
        Ok(pack)
    }
}

/// Input triangle soup of one mesh for [`MeshPack::add_mesh`]: vertices and, per triangle, its
/// three vertex indices and atlas page.
pub struct MeshInput {
    pub model: String,
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<([u32; 3], u16)>,
}

impl MeshPack {
    /// Appends a mesh: its triangles grouped by page, ordered along a Morton curve of their
    /// centroids (compact clusters), cut into clusters of [`CLUSTER_TRIANGLES`] and padded with
    /// degenerate triangles. Returns the mesh index.
    pub fn add_mesh(&mut self, m: MeshInput) -> u32 {
        let first_vertex = self.vertices.len() as u32;
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &m.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.pos[k]);
                hi[k] = hi[k].max(v.pos[k]);
            }
        }
        if m.vertices.is_empty() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        }
        let ext = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3), (hi[2] - lo[2]).max(1e-3)];
        let morton = |t: &[u32; 3]| -> u32 {
            let c: Vec<f32> = (0..3).map(|k| (m.vertices[t[0] as usize].pos[k] + m.vertices[t[1] as usize].pos[k] + m.vertices[t[2] as usize].pos[k]) / 3.0).collect();
            let q = |k: usize| (((c[k] - lo[k]) / ext[k]).clamp(0.0, 1.0) * 1023.0) as u32;
            let spread = |mut x: u32| {
                x &= 0x3FF;
                x = (x | (x << 16)) & 0x030000FF;
                x = (x | (x << 8)) & 0x0300F00F;
                x = (x | (x << 4)) & 0x030C30C3;
                (x | (x << 2)) & 0x09249249
            };
            spread(q(0)) | (spread(q(1)) << 1) | (spread(q(2)) << 2)
        };
        let mut tris: Vec<(u16, u32, [u32; 3])> = m.triangles.iter().map(|(t, p)| (*p, morton(t), *t)).collect();
        tris.sort_by_key(|(p, k, _)| (*p, *k));
        let mesh = self.meshes.len() as u32;
        let first_cluster = self.clusters.len() as u32;
        let mut i = 0;
        while i < tris.len() {
            let page = tris[i].0;
            let mut j = i;
            while j < tris.len() && tris[j].0 == page && j - i < CLUSTER_TRIANGLES {
                j += 1;
            }
            let first_index = self.indices.len() as u32;
            let mut cmin = [f32::MAX; 3];
            let mut cmax = [f32::MIN; 3];
            for (_, _, t) in &tris[i..j] {
                for &vi in t {
                    self.indices.push(first_vertex + vi);
                    let p = m.vertices[vi as usize].pos;
                    for k in 0..3 {
                        cmin[k] = cmin[k].min(p[k]);
                        cmax[k] = cmax[k].max(p[k]);
                    }
                }
            }
            let pad = first_vertex + tris[j - 1].2[2];
            for _ in (j - i)..CLUSTER_TRIANGLES {
                self.indices.extend([pad, pad, pad]);
            }
            self.clusters.push(Cluster { first_index, first_vertex, mesh, page, triangles: (j - i) as u8, flags: 0, min: cmin, max: cmax });
            i = j;
        }
        let triangles = tris.len() as u32;
        self.vertices.extend(m.vertices);
        let vertex_count = self.vertices.len() as u32 - first_vertex;
        self.meshes.push(Mesh { first_cluster, cluster_count: self.clusters.len() as u32 - first_cluster, first_vertex, vertex_count, triangles, bounds: [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]], model: m.model });
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vertex {
        Vertex { pos: [x, y, z], normal: pack_unit([0.0, 0.0, 1.0], 0), tangent: pack_unit([1.0, 0.0, 0.0], 1), uv: [100, 65535], color: [255, 128, 0, 255] }
    }

    #[test]
    fn clusters_pad_and_round_trip() {
        let mut pack = MeshPack::default();
        pack.pages.push(Page { width: 4096, height: 4096, mips: 13, alpha_threshold: 128, class: 0, albedo: "a.dds".into(), normal: "a_n.dds".into() });
        pack.pages.push(Page { width: 2048, height: 2048, mips: 12, alpha_threshold: 128, class: 2, albedo: "b.dds".into(), normal: "b_n.dds".into() });
        // 100 triangles on page 0, 30 on page 1 -> clusters 64 + 36 + 30.
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        for i in 0..130u32 {
            verts.extend([v(i as f32, 0.0, 0.0), v(i as f32, 1.0, 0.0), v(i as f32, 0.0, 1.0)]);
            tris.push(([3 * i, 3 * i + 1, 3 * i + 2], u16::from(i >= 100)));
        }
        let a = pack.add_mesh(MeshInput { model: "m1".into(), vertices: verts.clone(), triangles: tris.clone() });
        let b = pack.add_mesh(MeshInput { model: "m2".into(), vertices: verts[..3].to_vec(), triangles: vec![([0, 1, 2], 1)] });
        assert_eq!((a, b), (0, 1));
        assert_eq!(pack.clusters.len(), 4);
        assert_eq!(pack.clusters.iter().map(|c| c.triangles).collect::<Vec<_>>(), vec![64, 36, 30, 1]);
        assert_eq!(pack.clusters[2].page, 1);
        assert_eq!(pack.indices.len(), 4 * 192);
        assert_eq!(pack.meshes[1].first_vertex, 390);
        assert!(pack.indices[3 * 192..].iter().all(|&i| (390..393).contains(&i)));
        assert_eq!(pack.meshes[0].triangles, 130);
        let bytes = pack.to_bytes(WriteOptions { zstd: Some(3), crc: true }).unwrap();
        let back = MeshPack::from_bytes(&bytes).unwrap();
        assert_eq!(back, pack);
        assert_eq!(back.mesh_index("M2"), Some(1));
        let (n, w) = unpack_unit(back.vertices[0].tangent);
        assert!((n[0] - 1.0).abs() < 1e-3 && w == 1);
    }
}
