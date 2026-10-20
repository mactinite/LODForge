//! The `TVDT` field of an exterior CELL: one visibility bit per LOD block at every LOD level
//! over a fixed `stride x stride` cell grid (docs/re/occlusion-spec.md, section 4).
//!
//! The field is the pre-order traversal of a quadtree whose leaves are the 4x4-cell blocks:
//! a node's bit, then the subtrees of its south-west, north-west, south-east and north-east
//! children. Bit `p` is bit `p % 8` of byte `p / 8`. A set leaf means the block is visible from
//! the cell; every internal node is the OR of its children. For a stride of 256 the stream is
//! 5,461 bits in 684 bytes (683 bytes rounded up plus one spare byte, the 11 spare bits zero).
//!
//! The accessor design (node/leaf bit lookup, get/set by block) follows AnvilLOD's
//! `TvdtData.cs`; the bit assignment of the internal nodes is the one measured on the golden
//! output (section 4 of the spec), which differs from that file's description.
//!
//! Copyright (c) 2026 CageTV (AnvilLOD, `src/AnvilLOD.Core/Occlusion/TvdtData.cs`, commit
//! 606eece, GPL-3.0-only). Modified for LODForge: ported to Rust, internal-node layout taken
//! from the golden-verified specification, parametric stride, parent bits derived from leaves.
//! This file is part of LODForge and is distributed under the GNU General Public License,
//! version 3 only.

/// Quadtree layout of a grid of `stride` cells: level 0 = the `nb x nb` leaf blocks of 4x4
/// cells, level `levels - 1` = the whole grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Leaf blocks per side (`stride / 4`), a power of two.
    pub nb: usize,
    /// Number of levels (7 for stride 256).
    pub levels: u32,
    /// Bits in the stream (5,461 for stride 256).
    pub bits: usize,
    /// Field size in bytes (684 for stride 256).
    pub bytes: usize,
}

impl Layout {
    /// The layout of a lodsettings stride. The stride must be a power of two times four (the
    /// only observed value is 256; the field size rule for other strides is an assumption, see
    /// spec question 10.1).
    pub fn for_stride(stride: i32) -> Option<Layout> {
        if stride < 4 || stride % 4 != 0 {
            return None;
        }
        let nb = (stride / 4) as usize;
        if !nb.is_power_of_two() {
            return None;
        }
        let levels = nb.trailing_zeros() + 1;
        let bits = (0..levels).map(|l| 1usize << (2 * l)).sum::<usize>();
        Some(Layout { nb, levels, bits, bytes: bits.div_ceil(8) + 1 })
    }

    /// Nodes in a full subtree whose root has `height` (0 = a leaf): (4^(h+1) - 1) / 3.
    fn subtree(height: u32) -> usize {
        ((1usize << (2 * (height + 1))) - 1) / 3
    }

    /// Blocks per side at `level`.
    pub fn side(&self, level: u32) -> usize {
        self.nb >> level
    }

    /// Stream index (= bit position) of the node at `level` covering block (x, y) of that level.
    pub fn node_bit(&self, level: u32, x: usize, y: usize) -> usize {
        debug_assert!(level < self.levels && x < self.side(level) && y < self.side(level));
        let top = self.levels - 1;
        // Leaf coordinates of the node's south-west-most leaf; the child index at height k is
        // the k-th bit of each coordinate (x high, y low).
        let (lx, ly) = (x << level, y << level);
        let mut idx = 0;
        for h in (level + 1..=top).rev() {
            let k = h - 1;
            let child = (((lx >> k) & 1) << 1) | ((ly >> k) & 1);
            idx += 1 + child * Self::subtree(k);
        }
        idx
    }

    pub fn leaf_bit(&self, bx: usize, by: usize) -> usize {
        self.node_bit(0, bx, by)
    }
}

/// One cell's TVDT field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tvdt {
    pub layout: Layout,
    pub bytes: Vec<u8>,
}

impl Tvdt {
    pub fn empty(layout: Layout) -> Tvdt {
        Tvdt { layout, bytes: vec![0; layout.bytes] }
    }

    /// Wraps an existing field (any length; bits beyond the end read as clear).
    pub fn from_bytes(layout: Layout, bytes: Vec<u8>) -> Tvdt {
        Tvdt { layout, bytes }
    }

    pub fn bit(&self, p: usize) -> bool {
        self.bytes.get(p >> 3).is_some_and(|b| b >> (p & 7) & 1 == 1)
    }

    pub fn set_bit(&mut self, p: usize, v: bool) {
        if let Some(b) = self.bytes.get_mut(p >> 3) {
            if v {
                *b |= 1 << (p & 7);
            } else {
                *b &= !(1 << (p & 7));
            }
        }
    }

    pub fn node(&self, level: u32, x: usize, y: usize) -> bool {
        self.bit(self.layout.node_bit(level, x, y))
    }

    pub fn set_node(&mut self, level: u32, x: usize, y: usize, v: bool) {
        let p = self.layout.node_bit(level, x, y);
        self.set_bit(p, v);
    }

    pub fn leaf(&self, bx: usize, by: usize) -> bool {
        self.node(0, bx, by)
    }

    pub fn set_leaf(&mut self, bx: usize, by: usize, v: bool) {
        self.set_node(0, bx, by, v);
    }

    /// Sets every internal node to the OR of its four children (leaves first).
    pub fn fill_parents(&mut self) {
        for level in 1..self.layout.levels {
            let n = self.layout.side(level);
            for y in 0..n {
                for x in 0..n {
                    let any = self.node(level - 1, 2 * x, 2 * y) || self.node(level - 1, 2 * x + 1, 2 * y) || self.node(level - 1, 2 * x, 2 * y + 1) || self.node(level - 1, 2 * x + 1, 2 * y + 1);
                    self.set_node(level, x, y, any);
                }
            }
        }
    }

    /// Visible leaf blocks, indexed `bx * nb + by`.
    pub fn leaves(&self) -> Vec<bool> {
        let nb = self.layout.nb;
        let mut v = vec![false; nb * nb];
        for bx in 0..nb {
            for by in 0..nb {
                v[bx * nb + by] = self.leaf(bx, by);
            }
        }
        v
    }

    pub fn visible_leaves(&self) -> usize {
        self.leaves().iter().filter(|&&b| b).count()
    }

    /// Whether every internal node equals the OR of its children and no spare bit is set.
    pub fn is_consistent(&self) -> bool {
        for level in 1..self.layout.levels {
            let n = self.layout.side(level);
            for y in 0..n {
                for x in 0..n {
                    let any = self.node(level - 1, 2 * x, 2 * y) || self.node(level - 1, 2 * x + 1, 2 * y) || self.node(level - 1, 2 * x, 2 * y + 1) || self.node(level - 1, 2 * x + 1, 2 * y + 1);
                    if self.node(level, x, y) != any {
                        return false;
                    }
                }
            }
        }
        (self.layout.bits..self.bytes.len() * 8).all(|p| !self.bit(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_256_layout_matches_the_spec() {
        let l = Layout::for_stride(256).unwrap();
        assert_eq!((l.nb, l.levels, l.bits, l.bytes), (64, 7, 5461, 684));
        // Section 4: bit 0 root, 1..5 the south-west ancestors, 6 the south-west-most block, 7 the
        // block north of it, 8 east, 9 (1,1), 10 the 8x8 node at block (0,2), 11 the block (0,2).
        assert_eq!(l.node_bit(6, 0, 0), 0);
        assert_eq!(l.node_bit(5, 0, 0), 1);
        assert_eq!(l.node_bit(1, 0, 0), 5);
        assert_eq!(l.leaf_bit(0, 0), 6);
        assert_eq!(l.leaf_bit(0, 1), 7);
        assert_eq!(l.leaf_bit(1, 0), 8);
        assert_eq!(l.leaf_bit(1, 1), 9);
        assert_eq!(l.node_bit(1, 0, 1), 10);
        assert_eq!(l.leaf_bit(0, 2), 11);
        // Every node has a distinct bit inside the stream.
        let mut seen = vec![false; l.bits];
        for level in 0..l.levels {
            for y in 0..l.side(level) {
                for x in 0..l.side(level) {
                    let p = l.node_bit(level, x, y);
                    assert!(!seen[p], "bit {p} assigned twice");
                    seen[p] = true;
                }
            }
        }
        assert!(seen.iter().all(|&s| s));
        assert!(Layout::for_stride(192).is_none());
        assert_eq!(Layout::for_stride(4).unwrap().bits, 1);
    }

    #[test]
    fn parents_are_the_or_of_their_children() {
        let l = Layout::for_stride(256).unwrap();
        let mut t = Tvdt::empty(l);
        t.set_leaf(5, 9, true);
        t.set_leaf(63, 0, true);
        t.fill_parents();
        assert!(t.is_consistent());
        assert!(t.node(6, 0, 0) && t.node(1, 2, 4) && t.node(2, 1, 2) && !t.node(1, 0, 0) && t.node(1, 31, 0));
        assert_eq!(t.visible_leaves(), 2);
        assert_eq!(u32::from_le_bytes(t.bytes[..4].try_into().unwrap()) & 3, 3);
        t.set_bit(l.bits + 3, true);
        assert!(!t.is_consistent());
    }
}
