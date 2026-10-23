//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! The geometry of one part in shape-local space, as pruning and simplification work on it
//! (TreeLODGen clones the shape and edits it in place with nifly; spec 3.3). Per-vertex arrays are
//! kept only when the source has them for every vertex.

use lodforge_nif::TriShape;

#[derive(Debug, Clone, Default)]
pub struct PartMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 4]>,
    pub triangles: Vec<[u32; 3]>,
}

impl PartMesh {
    /// The shape's geometry; with `vertices`, only those (mixed shapes, spec 3.3).
    pub fn from_shape(s: &TriShape, vertices: Option<&[u32]>) -> PartMesh {
        let n = s.positions.len();
        let take = |len: usize| len == n;
        let mut m = PartMesh {
            positions: s.positions.clone(),
            uvs: if take(s.uvs.len()) { s.uvs.clone() } else { Vec::new() },
            normals: if take(s.normals.len()) { s.normals.clone() } else { Vec::new() },
            tangents: if take(s.tangents.len()) { s.tangents.clone() } else { Vec::new() },
            bitangents: if take(s.bitangents.len()) { s.bitangents.clone() } else { Vec::new() },
            colors: if take(s.colors.len()) { s.colors.clone() } else { Vec::new() },
            triangles: s.triangles.iter().map(|t| t.map(u32::from)).collect(),
        };
        if let Some(keep) = vertices {
            let mut remove = vec![true; n];
            for &v in keep {
                if let Some(r) = remove.get_mut(v as usize) {
                    *r = false;
                }
            }
            m.delete_vertices(&remove);
        }
        m
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Removes the flagged vertices and every triangle that uses one; the rest keep their
    /// relative order (nifly `DeleteVertsForShape`).
    pub fn delete_vertices(&mut self, remove: &[bool]) {
        let mut map = vec![u32::MAX; self.positions.len()];
        let mut next = 0u32;
        for (i, r) in remove.iter().enumerate().take(self.positions.len()) {
            if !r {
                map[i] = next;
                next += 1;
            }
        }
        fn keep<T: Copy>(v: &mut Vec<T>, remove: &[bool]) {
            if v.is_empty() {
                return;
            }
            let mut i = 0;
            v.retain(|_| {
                let k = !remove.get(i).copied().unwrap_or(false);
                i += 1;
                k
            });
        }
        keep(&mut self.positions, remove);
        keep(&mut self.uvs, remove);
        keep(&mut self.normals, remove);
        keep(&mut self.tangents, remove);
        keep(&mut self.bitangents, remove);
        keep(&mut self.colors, remove);
        self.triangles = self
            .triangles
            .iter()
            .filter(|t| t.iter().all(|&v| map.get(v as usize).is_some_and(|&m| m != u32::MAX)))
            .map(|t| t.map(|v| map[v as usize]))
            .collect();
    }

    /// Removes vertices no triangle uses.
    pub fn delete_unreferenced(&mut self) {
        let mut used = vec![false; self.positions.len()];
        for t in &self.triangles {
            for &v in t {
                used[v as usize] = true;
            }
        }
        let remove: Vec<bool> = used.iter().map(|u| !u).collect();
        self.delete_vertices(&remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_vertices_drops_their_triangles_and_renumbers() {
        let mut m = PartMesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
            ..Default::default()
        };
        m.delete_vertices(&[true, false, false, false]);
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.uvs[0], [1.0, 0.0]);
        assert_eq!(m.triangles, vec![[0, 2, 1]]);
    }
}
