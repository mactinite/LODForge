// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Edge dilation for impostor atlases, ported from Project Terra's `ImpostorDilation.cs`.
//!
//! Floods covered-texel colours outward into empty texels so bilinear filtering and mip
//! averaging never pull in background colour (the dark-halo artifact). Both images dilate
//! together under ONE coverage mask (the albedo's), because a texel is either tree or background
//! for both. Albedo keeps its alpha (coverage stays 0 outside the tree); the normal-depth image
//! dilates all four channels so depth continues outward too. Deterministic: each ring reads the
//! previous ring's state (ping-pong buffers), rows in parallel.

use rayon::prelude::*;

/// 8-neighbour dilation of two RGBA8 images (`width` x `height`), `iterations` rings. `covered`
/// is the coverage mask and is updated in place (texels filled by a ring become covered).
pub fn dilate(albedo: &mut [u8], normal_depth: &mut [u8], covered: &mut [bool], width: usize, height: usize, iterations: u32) {
    assert_eq!(albedo.len(), width * height * 4);
    assert_eq!(normal_depth.len(), width * height * 4);
    assert_eq!(covered.len(), width * height);
    if iterations == 0 {
        return;
    }
    let mut src_a = albedo.to_vec();
    let mut src_n = normal_depth.to_vec();
    let mut src_c = covered.to_vec();
    let mut dst_a = vec![0u8; src_a.len()];
    let mut dst_n = vec![0u8; src_n.len()];
    let mut dst_c = vec![false; src_c.len()];
    for _ in 0..iterations {
        let (sa, sn, sc) = (&src_a, &src_n, &src_c);
        dst_a
            .par_chunks_mut(width * 4)
            .zip(dst_n.par_chunks_mut(width * 4))
            .zip(dst_c.par_chunks_mut(width))
            .enumerate()
            .for_each(|(y, ((da, dn), dc))| {
                for x in 0..width {
                    let index = y * width + x;
                    let (o, s) = (x * 4, index * 4);
                    if sc[index] {
                        da[o..o + 4].copy_from_slice(&sa[s..s + 4]);
                        dn[o..o + 4].copy_from_slice(&sn[s..s + 4]);
                        dc[x] = true;
                        continue;
                    }
                    let mut count = 0u32;
                    let mut a = [0u32; 3];
                    let mut n = [0u32; 4];
                    for dy in -1i32..=1 {
                        let ny = y as i32 + dy;
                        if ny < 0 || ny >= height as i32 {
                            continue;
                        }
                        for dx in -1i32..=1 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }
                            let nx = x as i32 + dx;
                            if nx < 0 || nx >= width as i32 {
                                continue;
                            }
                            let ni = ny as usize * width + nx as usize;
                            if !sc[ni] {
                                continue;
                            }
                            count += 1;
                            for k in 0..3 {
                                a[k] += u32::from(sa[ni * 4 + k]);
                            }
                            for k in 0..4 {
                                n[k] += u32::from(sn[ni * 4 + k]);
                            }
                        }
                    }
                    if count == 0 {
                        da[o..o + 4].copy_from_slice(&sa[s..s + 4]);
                        dn[o..o + 4].copy_from_slice(&sn[s..s + 4]);
                        dc[x] = false;
                        continue;
                    }
                    // Integer division truncates, as Terra's byte casts do.
                    da[o..o + 3].copy_from_slice(&[(a[0] / count) as u8, (a[1] / count) as u8, (a[2] / count) as u8]);
                    da[o + 3] = sa[s + 3];
                    dn[o..o + 4].copy_from_slice(&[(n[0] / count) as u8, (n[1] / count) as u8, (n[2] / count) as u8, (n[3] / count) as u8]);
                    dc[x] = true;
                }
            });
        std::mem::swap(&mut src_a, &mut dst_a);
        std::mem::swap(&mut src_n, &mut dst_n);
        std::mem::swap(&mut src_c, &mut dst_c);
    }
    albedo.copy_from_slice(&src_a);
    normal_depth.copy_from_slice(&src_n);
    covered.copy_from_slice(&src_c);
}

/// [`dilate`] of one `frame` x `frame` cell at (`x0`, `y0`) of two atlases `stride` texels wide:
/// the cell is dilated on its own, so no frame floods into its neighbours.
pub fn dilate_cell(albedo: &mut [u8], normal_depth: &mut [u8], stride: usize, x0: usize, y0: usize, frame: usize, iterations: u32) {
    let mut a = vec![0u8; frame * frame * 4];
    let mut n = vec![0u8; frame * frame * 4];
    for y in 0..frame {
        let s = ((y0 + y) * stride + x0) * 4;
        a[y * frame * 4..(y + 1) * frame * 4].copy_from_slice(&albedo[s..s + frame * 4]);
        n[y * frame * 4..(y + 1) * frame * 4].copy_from_slice(&normal_depth[s..s + frame * 4]);
    }
    let mut covered: Vec<bool> = a.chunks_exact(4).map(|p| p[3] > 0).collect();
    dilate(&mut a, &mut n, &mut covered, frame, frame, iterations);
    for y in 0..frame {
        let s = ((y0 + y) * stride + x0) * 4;
        albedo[s..s + frame * 4].copy_from_slice(&a[y * frame * 4..(y + 1) * frame * 4]);
        normal_depth[s..s + frame * 4].copy_from_slice(&n[y * frame * 4..(y + 1) * frame * 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floods_covered_color_into_empty_neighbors_preserving_albedo_alpha() {
        let (w, h) = (3, 3);
        let mut albedo = vec![0u8; w * h * 4];
        let mut nd: Vec<u8> = (0..w * h).flat_map(|_| [128, 128, 255, 255]).collect();
        let mut covered = vec![false; w * h];
        albedo[16..20].copy_from_slice(&[200, 100, 50, 255]); // centre covered
        nd[16..20].copy_from_slice(&[10, 20, 30, 40]);
        covered[4] = true;
        dilate(&mut albedo, &mut nd, &mut covered, w, h, 1);
        for i in 0..w * h {
            assert_eq!(albedo[i * 4], 200, "texel {i} rgb flooded");
            assert_eq!(nd[i * 4 + 3], 40, "texel {i} normal-depth alpha flooded");
            assert!(covered[i]);
        }
        assert_eq!(albedo[16 + 3], 255, "covered texel alpha untouched");
        assert_eq!(albedo[3], 0, "empty texel coverage stays 0");
    }

    #[test]
    fn zero_iterations_is_a_no_op() {
        let mut albedo = vec![1, 2, 3, 0, 9, 8, 7, 255];
        let mut nd = vec![4, 5, 6, 7, 1, 1, 1, 1];
        let mut covered = vec![false, true];
        dilate(&mut albedo, &mut nd, &mut covered, 2, 1, 0);
        assert_eq!(albedo[0], 1);
        assert_eq!(nd[0], 4);
        assert!(!covered[0]);
    }

    #[test]
    fn rings_grow_one_texel_per_iteration_and_average_neighbours() {
        let w = 7;
        let mut albedo = vec![0u8; w * 4];
        let mut nd = vec![0u8; w * 4];
        let mut covered = vec![false; w];
        albedo[0..4].copy_from_slice(&[100, 0, 0, 255]);
        albedo[12..16].copy_from_slice(&[200, 0, 0, 255]);
        covered[0] = true;
        covered[3] = true;
        dilate(&mut albedo, &mut nd, &mut covered, w, 1, 2);
        // Ring 1 fills texel 1 (from 0), 2 and 4 (from 3); ring 2 fills 5 (from 4).
        assert_eq!(albedo[4], 100);
        assert_eq!(albedo[8], 200);
        assert_eq!(albedo[16], 200);
        assert_eq!(albedo[20], 200);
        assert!(!covered[6]);
        assert_eq!(albedo[24], 0);
    }

    #[test]
    fn cells_do_not_bleed_into_each_other() {
        // Two 2x2 cells side by side in a 4x2 atlas; only the left one has a covered texel.
        let mut albedo = vec![0u8; 4 * 2 * 4];
        let mut nd = vec![0u8; 4 * 2 * 4];
        albedo[0..4].copy_from_slice(&[50, 60, 70, 255]);
        dilate_cell(&mut albedo, &mut nd, 4, 0, 0, 2, 4);
        dilate_cell(&mut albedo, &mut nd, 4, 2, 0, 2, 4);
        assert_eq!(albedo[4], 50, "left cell flooded");
        assert_eq!(albedo[2 * 4], 0, "right cell untouched");
    }
}
