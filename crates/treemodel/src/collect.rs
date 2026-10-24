//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Source shapes of a tree NIF (TreeLODGen `Split.SourceShapes`, `NifUtil.GlobalTransform`;
//! spec 2.1-2.2).
//!
//! Every triangle shape with triangles, as TreeLODGen takes them (no filtering of hidden shapes
//! or effect shaders: spec "port risks" 1), except that a switch node contributes its first child
//! with geometry only. Vanilla trees hold the
//! geometry twice, skinned for wind and static for distance; the static copy is used when its
//! triangles are at least half the skinned ones', else the skinned one (wind-only mods).
//! Transforms: every node between the shape and the root, the root's own transform left out (the
//! game replaces it with the reference's placement; Ulvenwald scales trees there).

use lodforge_nif::{Block, Nif};

use crate::xf::Xf;

#[derive(Debug, Clone)]
pub struct SourceShape {
    /// Block index of the shape.
    pub block: usize,
    pub name: String,
    /// Shape transform with its parents', without the root's.
    pub xf: Xf,
    pub skinned: bool,
    pub triangles: usize,
}

/// The shapes to build the LOD from, in file order.
pub fn collect(nif: &Nif) -> Vec<SourceShape> {
    let mut all = Vec::new();
    if let Some(Block::Node(root)) = nif.blocks.first() {
        for c in root.children.iter().flatten() {
            walk(nif, *c as usize, Xf::IDENTITY, &mut all);
        }
    }
    all.retain(|s| s.triangles > 0);
    let tris = |skinned: bool| all.iter().filter(|s| s.skinned == skinned).map(|s| s.triangles).sum::<usize>();
    let (fixed, skinned) = (tris(false), tris(true));
    if fixed == 0 || skinned == 0 {
        return all;
    }
    let use_skinned = fixed * 2 < skinned;
    all.retain(|s| s.skinned == use_skinned);
    all
}

fn walk(nif: &Nif, idx: usize, parent: Xf, out: &mut Vec<SourceShape>) {
    match nif.blocks.get(idx) {
        Some(Block::Node(node)) => {
            let xf = Xf::of(&node.av.transform).then(&parent);
            // A switch node shows one child. Divergence from TreeLODGen, which takes them all:
            // vanilla trees keep a second, static copy of every shape there, and both copies made
            // the crown twice as dense.
            // The first child with geometry: vanilla trees put the skinned copy first, whose
            // triangles live in the skin partitions and do not count here.
            if nif.block_type(idx) == "NiSwitchNode" {
                for c in node.children.iter().flatten() {
                    let mut sub = Vec::new();
                    walk(nif, *c as usize, xf, &mut sub);
                    if sub.iter().any(|s| s.triangles > 0) {
                        out.extend(sub);
                        break;
                    }
                }
                return;
            }
            for c in node.children.iter().flatten() {
                walk(nif, *c as usize, xf, out);
            }
        }
        Some(Block::TriShape(s)) => out.push(SourceShape {
            block: idx,
            name: s.av.net.name.clone().unwrap_or_default(),
            xf: Xf::of(&s.av.transform).then(&parent),
            skinned: s.skin.is_some(),
            triangles: s.triangles.len(),
        }),
        _ => {}
    }
}
