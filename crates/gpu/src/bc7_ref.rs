//! Scalar Rust copy of `bc7_ispc.wgsl` (same operations, IEEE f32, `mul_add` where the shader
//! uses `fma`): an oracle for checking the shader and its assumptions against the ISPC encoder
//! without the GPU's own arithmetic in the way. Not used by the encoders.
#![allow(clippy::needless_range_loop)]

use crate::bc7_tables::{PATTERN, PATTERN_MASK, SKIP, UNQUANT};
use intel_tex_2::bc7::EncodeSettings;

const BIG: f32 = 3.0e38;

fn cvt(x: f32) -> i32 {
    if ((x.to_bits() >> 23) & 0xff) >= 158 { i32::MIN } else { x as i32 }
}

fn fma(a: f32, b: f32, c: f32) -> f32 {
    a.mul_add(b, c)
}

fn get_pattern_mask(part_id: i32, j: i32) -> i32 {
    let packed = PATTERN_MASK[part_id as usize];
    let m0 = (packed & 0xFFFF) as i32;
    let m1 = (packed >> 16) as i32;
    match j {
        2 => !m0 & !m1,
        0 => m0,
        _ => m1,
    }
}

fn get_skip(part_id: i32, j: i32) -> i32 {
    let p = SKIP[part_id as usize] as i32;
    match j {
        0 => 0,
        1 => p >> 4,
        _ => p & 15,
    }
}

fn unquant(bits: i32, i: i32) -> i32 {
    let base = match bits {
        2 => 0,
        3 => 4,
        _ => 12,
    };
    UNQUANT[base + i as usize] as i32
}

fn unpack_to_byte(v: i32, bits: i32) -> i32 {
    let vv = v << (8 - bits);
    vv + (vv >> bits)
}

fn pairs_of(mode: i32) -> i32 {
    match mode {
        0 | 2 => 3,
        4..=6 => 1,
        _ => 2,
    }
}

struct St {
    s: EncodeSettings,
    blk: [f32; 64],
    cur: [f32; 64],
    best_err: f32,
    opaque_err: f32,
    best_data: [u32; 5],
    ep: [f32; 24],
    qep: [i32; 24],
    qb: [u32; 2],
    cand_qep: [i32; 8],
    cand_qblock: [u32; 2],
    cand_aqep: [i32; 2],
    cand_aqblock: [u32; 2],
    cand_rotation: i32,
    cand_swap: i32,
}

fn sq(v: f32) -> f32 {
    v * v
}

impl St {
    fn refine(&self, mode: i32) -> i32 {
        self.s.refine_iterations[mode as usize] as i32
    }

    fn compute_stats_masked(&self, mask: i32, channels: i32) -> [f32; 15] {
        let mut s = [0f32; 15];
        for k in 0..16 {
            let flag = ((mask >> k) & 1) as f32;
            let mut c = [0f32; 4];
            for p in 0..channels as usize {
                c[p] = self.cur[k + p * 16] * flag;
            }
            s[14] += flag;
            s[10] += c[0];
            s[11] += c[1];
            s[12] += c[2];
            s[0] = fma(c[0], c[0], s[0]);
            s[1] = fma(c[0], c[1], s[1]);
            s[2] = fma(c[0], c[2], s[2]);
            s[4] = fma(c[1], c[1], s[4]);
            s[5] = fma(c[1], c[2], s[5]);
            s[7] = fma(c[2], c[2], s[7]);
            if channels == 4 {
                s[13] += c[3];
                s[3] = fma(c[0], c[3], s[3]);
                s[6] = fma(c[1], c[3], s[6]);
                s[8] = fma(c[2], c[3], s[8]);
                s[9] = fma(c[3], c[3], s[9]);
            }
        }
        s
    }

    fn block_segment(&mut self, base: usize, mask: i32, channels: i32) {
        let s = self.compute_stats_masked(mask, channels);
        let mut c = covar_from_stats(&s, channels);
        let mut dc = [0f32; 4];
        for p in 0..channels as usize {
            dc[p] = s[10 + p] / s[14];
        }
        for k in 0..10 {
            c[k] *= 1.0 / 65536.0;
        }
        let eps = f32::from_bits(0x358637BD);
        c[0] += eps;
        c[4] += eps;
        c[7] += eps;
        c[9] += eps;
        let axis = compute_axis(&c, 8, channels);
        let (mut lo, mut hi) = (BIG, -BIG);
        for k in 0..16 {
            if (mask >> k) & 1 == 0 {
                continue;
            }
            let mut dot = 0.0f32;
            for p in 0..channels as usize {
                dot = fma(axis[p], self.cur[16 * p + k] - dc[p], dot);
            }
            lo = lo.min(dot);
            hi = hi.max(dot);
        }
        if hi - lo < 1.0 {
            lo -= 0.5;
            hi += 0.5;
        }
        for p in 0..channels as usize {
            self.ep[base + p] = fma(lo, axis[p], dc[p]).clamp(0.0, 255.0);
            self.ep[base + 4 + p] = fma(hi, axis[p], dc[p]).clamp(0.0, 255.0);
        }
    }

    fn ep_quant0367(&mut self, base: usize, mode: i32, channels: i32) {
        let bits = match mode {
            0 => 4,
            7 => 5,
            _ => 7,
        };
        let levels = 1 << bits;
        let levels2 = levels * 2 - 1;
        for i in 0..2 {
            let mut q = [0i32; 8];
            for b in 0..2 {
                for p in 0..4 {
                    let t = fma(self.ep[base + i * 4 + p] / 255.0, levels2 as f32, -(b as f32));
                    let v = cvt(t * 0.5 + 0.5) * 2 + b;
                    q[(b * 4) as usize + p] = v.clamp(b, levels2 - 1 + b);
                }
            }
            let mut e = [0f32; 8];
            for j in 0..8 {
                e[j] = if mode == 0 { unpack_to_byte(q[j], 5) as f32 } else { q[j] as f32 };
            }
            let (mut err0, mut err1) = (0f32, 0f32);
            for p in 0..channels as usize {
                let d0 = self.ep[base + i * 4 + p] - e[p];
                let d1 = self.ep[base + i * 4 + p] - e[4 + p];
                err0 = fma(d0, d0, err0);
                err1 = fma(d1, d1, err1);
            }
            for p in 0..4 {
                self.qep[base + i * 4 + p] = if err0 < err1 { q[p] } else { q[4 + p] };
            }
        }
    }

    fn ep_quant1(&mut self, base: usize) {
        let mut q = [0i32; 16];
        for b in 0..2 {
            for i in 0..8 {
                let t = fma(self.ep[base + i] / 255.0, 127.0, -(b as f32));
                let v = cvt(t * 0.5 + 0.5) * 2 + b;
                q[(b * 8) as usize + i] = v.clamp(b, 126 + b);
            }
        }
        let e: Vec<f32> = q.iter().map(|&v| unpack_to_byte(v, 7) as f32).collect();
        let (mut err0, mut err1) = (0f32, 0f32);
        for j in 0..2 {
            for p in 0..3 {
                let d0 = self.ep[base + j * 4 + p] - e[j * 4 + p];
                let d1 = self.ep[base + j * 4 + p] - e[8 + j * 4 + p];
                err0 = fma(d0, d0, err0);
                err1 = fma(d1, d1, err1);
            }
        }
        for i in 0..8 {
            self.qep[base + i] = if err0 < err1 { q[i] } else { q[8 + i] };
        }
    }

    fn ep_quant245(&mut self, base: usize, mode: i32) {
        let bits = if mode == 5 { 7 } else { 5 };
        let levels = 1 << bits;
        for i in 0..8 {
            let v = cvt(fma(self.ep[base + i] / 255.0, (levels - 1) as f32, 0.5));
            self.qep[base + i] = v.clamp(0, levels - 1);
        }
    }

    fn ep_quant_dequant(&mut self, mode: i32, channels: i32) {
        let pairs = pairs_of(mode) as usize;
        for i in 0..pairs {
            match mode {
                0 | 3 | 6 | 7 => self.ep_quant0367(i * 8, mode, channels),
                1 => self.ep_quant1(i * 8),
                _ => self.ep_quant245(i * 8, mode),
            }
        }
        for i in 0..8 * pairs {
            self.ep[i] = match mode {
                3 | 6 => self.qep[i] as f32,
                1 | 5 => unpack_to_byte(self.qep[i], 7) as f32,
                7 => unpack_to_byte(self.qep[i], 6) as f32,
                _ => unpack_to_byte(self.qep[i], 5) as f32,
            };
        }
    }

    fn block_quant(&mut self, bits: i32, pattern: u32, channels: i32) -> f32 {
        let mut total = 0f32;
        let levels = 1 << bits;
        self.qb = [0, 0];
        let mut ps = pattern;
        for k in 0..16 {
            let j = (ps & 3) as usize;
            ps >>= 2;
            let (mut proj, mut div) = (0f32, 0f32);
            for p in 0..channels as usize {
                let a = self.ep[8 * j + p];
                let b = self.ep[8 * j + 4 + p];
                proj = fma(self.cur[k + p * 16] - a, b - a, proj);
                div = fma(b - a, b - a, div);
            }
            proj /= div;
            let q1 = cvt(fma(proj, levels as f32, 0.5)).clamp(1, levels - 1);
            let (w0, w1) = (unquant(bits, q1 - 1), unquant(bits, q1));
            let (mut err0, mut err1) = (0f32, 0f32);
            for p in 0..channels as usize {
                let a = self.ep[8 * j + p];
                let b = self.ep[8 * j + 4 + p];
                let d0 = cvt((fma((64 - w0) as f32, a, w0 as f32 * b) + 32.0) * 0.015625) as f32 - self.cur[k + p * 16];
                let d1 = cvt((fma((64 - w1) as f32, a, w1 as f32 * b) + 32.0) * 0.015625) as f32 - self.cur[k + p * 16];
                err0 = fma(d0, d0, err0);
                err1 = fma(d1, d1, err1);
            }
            let (mut be, mut bq) = (cvt(err1), q1);
            if err0 < err1 {
                be = cvt(err0);
                bq = q1 - 1;
            }
            self.qb[k / 8] += (bq as u32) << (4 * (k % 8));
            total += be as f32;
        }
        total
    }

    fn opt_endpoints(&mut self, base: usize, bits: i32, mask: i32, channels: i32) {
        let levels = 1 << bits;
        let mut atb1 = [0f32; 4];
        let (mut sum_q, mut sum_qq) = (0f32, 0f32);
        let mut sum = [0f32; 5];
        for k1 in 0..2 {
            let mut qs = self.qb[k1];
            for k2 in 0..8 {
                let k = k1 * 8 + k2;
                let q = (qs & 15) as i32 as f32;
                qs >>= 4;
                if (mask >> k) & 1 == 0 {
                    continue;
                }
                let x = cvt((levels - 1) as f32 - q);
                sum_q += q;
                sum_qq = fma(q, q, sum_qq);
                sum[4] += 1.0;
                for p in 0..channels as usize {
                    sum[p] += self.cur[k + p * 16];
                }
                for p in 0..channels as usize {
                    atb1[p] = fma(x as f32, self.cur[k + p * 16], atb1[p]);
                }
            }
        }
        let mut atb2 = [0f32; 4];
        let l1 = (levels - 1) as f32;
        for p in 0..channels as usize {
            atb2[p] = fma(l1, sum[p], -atb1[p]);
        }
        let cxx = fma(sum[4], sq(l1), -((2 * (levels - 1)) as f32 * sum_q)) + sum_qq;
        let cyy = sum_qq;
        let cxy = fma(l1, sum_q, -sum_qq);
        let det = fma(cxx, cyy, -(cxy * cxy));
        let scale = l1 / det;
        for p in 0..channels as usize {
            self.ep[base + p] = fma(atb1[p], cyy, -(atb2[p] * cxy)) * scale;
            self.ep[base + 4 + p] = fma(atb2[p], cxx, -(atb1[p] * cxy)) * scale;
        }
        if det.abs() < 0.001 {
            for p in 0..channels as usize {
                self.ep[base + p] = sum[p] / sum[4];
                self.ep[base + 4 + p] = self.ep[base + p];
            }
        }
    }

    fn put_bits(&mut self, pos: &mut i32, bits: i32, v: u32) {
        let p = *pos;
        self.best_data[(p / 32) as usize] |= v << (p % 32);
        if p % 32 + bits > 32 {
            self.best_data[(p / 32 + 1) as usize] |= v >> (32 - p % 32);
        }
        *pos = p + bits;
    }

    fn data_shl_1bit_from(&mut self, at: i32) {
        let d = &mut self.best_data;
        if at < 96 {
            let shifted = (d[2] >> 1) | (d[3] << 31);
            let mask = ((1u32 << (at - 64)) - 1) >> 1;
            d[2] = (mask & d[2]) | (!mask & shifted);
            d[3] = (d[3] >> 1) | (d[4] << 31);
            d[4] >>= 1;
        } else if at < 128 {
            let shifted = (d[3] >> 1) | (d[4] << 31);
            let mask = ((1u32 << (at - 96)) - 1) >> 1;
            d[3] = (mask & d[3]) | (!mask & shifted);
            d[4] >>= 1;
        }
    }

    fn code_qblock(&mut self, pos: &mut i32, q0: u32, q1: u32, bits: i32, flips: i32) {
        let levels = 1 << bits;
        let mut fl = flips;
        for k1 in 0..2 {
            let mut qs = if k1 == 0 { q0 } else { q1 };
            for k2 in 0..8 {
                let mut q = (qs & 15) as i32;
                if fl & 1 > 0 {
                    q = (levels - 1) - q;
                }
                let n = if k1 == 0 && k2 == 0 { bits - 1 } else { bits };
                self.put_bits(pos, n, q as u32);
                qs >>= 4;
                fl >>= 1;
            }
        }
    }

    fn code_mode01237(&mut self, q_in: [i32; 24], qblock: [u32; 2], part_id: i32, mode: i32) {
        let bits = if mode == 0 || mode == 1 { 3 } else { 2 };
        let pairs = pairs_of(mode);
        let channels = if mode == 7 { 4 } else { 3 };
        let mut q = q_in;
        let mut flips = 0;
        let levels = 1 << bits;
        for j in 0..pairs {
            let k0 = get_skip(part_id, j);
            let qv = ((qblock[(k0 >> 3) as usize] << (28 - (k0 & 7) * 4)) >> 28) as i32;
            if qv >= levels / 2 {
                for p in 0..4 {
                    q.swap((8 * j + p) as usize, (8 * j + 4 + p) as usize);
                }
                flips |= get_pattern_mask(part_id, j);
            }
        }
        self.best_data = [0; 5];
        let mut pos = 0;
        self.put_bits(&mut pos, mode + 1, 1 << mode);
        if mode == 0 {
            self.put_bits(&mut pos, 4, (part_id & 15) as u32);
        } else {
            self.put_bits(&mut pos, 6, (part_id & 63) as u32);
        }
        for p in 0..channels {
            for j in 0..pairs * 2 {
                let v = q[(j * 4 + p) as usize];
                match mode {
                    0 => self.put_bits(&mut pos, 4, (v >> 1) as u32),
                    1 => self.put_bits(&mut pos, 6, (v >> 1) as u32),
                    2 => self.put_bits(&mut pos, 5, v as u32),
                    3 => self.put_bits(&mut pos, 7, (v >> 1) as u32),
                    _ => self.put_bits(&mut pos, 5, (v >> 1) as u32),
                }
            }
        }
        if mode == 1 {
            for j in 0..2 {
                self.put_bits(&mut pos, 1, (q[j * 8] & 1) as u32);
            }
        }
        if mode == 0 || mode == 3 || mode == 7 {
            for j in 0..pairs * 2 {
                self.put_bits(&mut pos, 1, (q[(j * 4) as usize] & 1) as u32);
            }
        }
        self.code_qblock(&mut pos, qblock[0], qblock[1], bits, flips);
        let (mut s1, mut s2) = (get_skip(part_id, 1), get_skip(part_id, 2));
        if pairs > 2 && s1 < s2 {
            std::mem::swap(&mut s1, &mut s2);
        }
        self.data_shl_1bit_from(128 + (pairs - 1) - (15 - s1) * bits);
        if pairs > 2 {
            self.data_shl_1bit_from(128 + (pairs - 1) - (15 - s2) * bits);
        }
    }

    fn code_mode6(&mut self, q_in: [i32; 8], qb_in: [u32; 2]) {
        let (mut q, mut qblock) = (q_in, qb_in);
        swap456(&mut q, 4, &mut qblock, 4);
        self.best_data = [0; 5];
        let mut pos = 0;
        self.put_bits(&mut pos, 7, 64);
        for p in 0..4 {
            self.put_bits(&mut pos, 7, (q[p] >> 1) as u32);
            self.put_bits(&mut pos, 7, (q[4 + p] >> 1) as u32);
        }
        self.put_bits(&mut pos, 1, (q[0] & 1) as u32);
        self.put_bits(&mut pos, 1, (q[4] & 1) as u32);
        self.code_qblock(&mut pos, qblock[0], qblock[1], 4, 0);
    }

    fn enc_mode01237(&mut self, mode: i32, list: &[i32; 64], part_count: i32) {
        if part_count == 0 {
            return;
        }
        let bits = if mode == 0 || mode == 1 { 3 } else { 2 };
        let pairs = pairs_of(mode);
        let channels = if mode == 7 { 4 } else { 3 };
        let (mut best_qep, mut best_qblock, mut best_part, mut best) = ([0i32; 24], [0u32; 2], -1, BIG);
        for part in 0..part_count as usize {
            let mut part_id = list[part] & 63;
            if pairs == 3 {
                part_id += 64;
            }
            for j in 0..pairs {
                self.block_segment((j * 8) as usize, get_pattern_mask(part_id, j), channels);
            }
            self.ep_quant_dequant(mode, channels);
            let err = self.block_quant(bits, PATTERN[part_id as usize], channels);
            if err < best {
                best_qep[..(8 * pairs) as usize].copy_from_slice(&self.qep[..(8 * pairs) as usize]);
                best_qblock = self.qb;
                best_part = part_id;
                best = err;
            }
        }
        for _ in 0..self.refine(mode) {
            self.qb = best_qblock;
            for j in 0..pairs {
                self.opt_endpoints((j * 8) as usize, bits, get_pattern_mask(best_part, j), channels);
            }
            self.ep_quant_dequant(mode, channels);
            let err = self.block_quant(bits, PATTERN[best_part as usize], channels);
            if err < best {
                best_qep[..(8 * pairs) as usize].copy_from_slice(&self.qep[..(8 * pairs) as usize]);
                best_qblock = self.qb;
                best = err;
            }
        }
        if mode != 7 {
            best += self.opaque_err;
        }
        if best < self.best_err {
            self.best_err = best;
            self.code_mode01237(best_qep, best_qblock, best_part, mode);
        }
    }

    fn bound_list(&self, channels: i32) -> [i32; 64] {
        let full = self.compute_stats_masked(-1, channels);
        let mut list = [0i32; 64];
        for part in 0..64 {
            let mask = get_pattern_mask(part, 0);
            let mut s = self.compute_stats_masked(mask, channels);
            let c1 = covar_from_stats(&s, channels);
            for i in 0..15 {
                s[i] = full[i] - s[i];
            }
            let c2 = covar_from_stats(&s, channels);
            let mut bound = 0f32;
            bound += get_pca_bound(c1, channels);
            bound += get_pca_bound(c2, channels);
            let b = cvt(bound.sqrt() * 256.0);
            list[part as usize] = part.wrapping_add(b.wrapping_mul(64));
        }
        list
    }

    fn channel_opt_quant(&self, qblock: &mut [u32; 2], ch: usize, bits: i32, e: [f32; 2]) -> f32 {
        let levels = 1 << bits;
        *qblock = [0, 0];
        let mut total = 0f32;
        for k in 0..16 {
            let v = self.blk[ch * 16 + k];
            let proj = (v - e[0]) / ((e[1] - e[0]) + 0.001);
            let q1 = cvt(fma(proj, levels as f32, 0.5)).clamp(1, levels - 1);
            let (w0, w1) = (unquant(bits, q1 - 1), unquant(bits, q1));
            let d0 = cvt((fma((64 - w0) as f32, e[0], w0 as f32 * e[1]) + 32.0) * 0.015625) as f32 - v;
            let d1 = cvt((fma((64 - w1) as f32, e[0], w1 as f32 * e[1]) + 32.0) * 0.015625) as f32 - v;
            let (err0, err1) = (d0 * d0, d1 * d1);
            let (mut be, mut bq) = (cvt(err1), q1);
            if err0 < err1 {
                be = cvt(err0);
                bq = q1 - 1;
            }
            qblock[k / 8] += (bq as u32) << (4 * (k % 8));
            total += be as f32;
        }
        total
    }

    fn opt_channel(&self, qblock: &mut [u32; 2], q: &mut [i32; 2], ch: usize, bits: i32, epbits: i32) -> f32 {
        let mut e = [255f32, 0.0];
        for k in 0..16 {
            e[0] = e[0].min(self.blk[ch * 16 + k]);
            e[1] = e[1].max(self.blk[ch * 16 + k]);
        }
        channel_quant_dequant(&mut e, q, epbits);
        let mut err = self.channel_opt_quant(qblock, ch, bits, e);
        for _ in 0..self.s.refine_iterations_channel {
            self.channel_opt_endpoints(&mut e, ch, bits, qblock[0], qblock[1]);
            channel_quant_dequant(&mut e, q, epbits);
            err = self.channel_opt_quant(qblock, ch, bits, e);
        }
        err
    }

    fn channel_opt_endpoints(&self, e: &mut [f32; 2], ch: usize, bits: i32, q0: u32, q1: u32) {
        let levels = 1 << bits;
        let (mut atb1, mut sum_q, mut sum_qq, mut sum) = (0f32, 0f32, 0f32, 0f32);
        for k1 in 0..2 {
            let mut qs = if k1 == 0 { q0 } else { q1 };
            for k2 in 0..8 {
                let k = k1 * 8 + k2;
                let q = (qs & 15) as i32 as f32;
                qs >>= 4;
                let x = cvt((levels - 1) as f32 - q);
                sum_q += q;
                sum_qq = fma(q, q, sum_qq);
                sum += self.blk[ch * 16 + k];
                atb1 = fma(x as f32, self.blk[ch * 16 + k], atb1);
            }
        }
        let l1 = (levels - 1) as f32;
        let atb2 = fma(l1, sum, -atb1);
        let cxx = fma(16.0, sq(l1), -((2 * (levels - 1)) as f32 * sum_q)) + sum_qq;
        let cyy = sum_qq;
        let cxy = fma(l1, sum_q, -sum_qq);
        let det = fma(cxx, cyy, -(cxy * cxy));
        let scale = l1 / det;
        e[0] = (fma(atb1, cyy, -(atb2 * cxy)) * scale).clamp(0.0, 255.0);
        e[1] = (fma(atb2, cxx, -(atb1 * cxy)) * scale).clamp(0.0, 255.0);
        if det.abs() < 0.001 {
            e[0] = sum / 16.0;
            e[1] = e[0];
        }
    }

    fn mode45_candidate(&mut self, best: &mut f32, mode: i32, rotation: i32, swap: i32) {
        let (mut bits, mut abits) = (2, if mode == 4 { 3 } else { 2 });
        let aepbits = if mode == 4 { 6 } else { 8 };
        if swap == 1 {
            bits = 3;
            abits = 2;
        }
        for k in 0..16 {
            for p in 0..3 {
                self.cur[k + p * 16] = self.blk[k + p * 16];
            }
            if rotation < 3 {
                if self.s.channels == 4 {
                    self.cur[k + rotation as usize * 16] = self.blk[k + 48];
                }
                if self.s.channels == 3 {
                    self.cur[k + rotation as usize * 16] = 255.0;
                }
            }
        }
        self.block_segment(0, -1, 3);
        self.ep_quant_dequant(mode, 3);
        let mut err = self.block_quant(bits, 0, 3);
        for _ in 0..self.refine(mode) {
            self.opt_endpoints(0, bits, -1, 3);
            self.ep_quant_dequant(mode, 3);
            err = self.block_quant(bits, 0, 3);
        }
        let (mut aqep, mut aqblock) = ([0i32; 2], [0u32; 2]);
        err += self.opt_channel(&mut aqblock, &mut aqep, rotation as usize, abits, aepbits);
        if err < *best {
            self.cand_qep.copy_from_slice(&self.qep[..8]);
            self.cand_qblock = self.qb;
            self.cand_aqep = aqep;
            self.cand_aqblock = aqblock;
            self.cand_rotation = rotation;
            self.cand_swap = swap;
            *best = err;
        }
    }

    fn code_mode45(&mut self, mode: i32) {
        let mut q = self.cand_qep;
        let mut qblock = self.cand_qblock;
        let mut aq = [self.cand_aqep[0], self.cand_aqep[1], 0, 0, 0, 0, 0, 0];
        let mut aqblock = self.cand_aqblock;
        let bits = 2;
        let abits = if mode == 4 { 3 } else { 2 };
        let epbits = if mode == 4 { 5 } else { 7 };
        let aepbits = if mode == 4 { 6 } else { 8 };
        if self.cand_swap == 0 {
            swap456(&mut q, 4, &mut qblock, bits);
            swap456(&mut aq, 1, &mut aqblock, abits);
        } else {
            std::mem::swap(&mut qblock, &mut aqblock);
            swap456(&mut aq, 1, &mut qblock, bits);
            swap456(&mut q, 4, &mut aqblock, abits);
        }
        self.best_data = [0; 5];
        let mut pos = 0;
        self.put_bits(&mut pos, mode + 1, 1 << mode);
        self.put_bits(&mut pos, 2, ((self.cand_rotation + 1) & 3) as u32);
        if mode == 4 {
            self.put_bits(&mut pos, 1, self.cand_swap as u32);
        }
        for p in 0..3 {
            self.put_bits(&mut pos, epbits, q[p] as u32);
            self.put_bits(&mut pos, epbits, q[4 + p] as u32);
        }
        self.put_bits(&mut pos, aepbits, aq[0] as u32);
        self.put_bits(&mut pos, aepbits, aq[1] as u32);
        self.code_qblock(&mut pos, qblock[0], qblock[1], bits, 0);
        self.code_qblock(&mut pos, aqblock[0], aqblock[1], abits, 0);
    }

    fn enc_mode45(&mut self) {
        let mut best = self.best_err;
        self.cand_qep = [0; 8];
        self.cand_qblock = [0; 2];
        self.cand_aqep = [0; 2];
        self.cand_aqblock = [0; 2];
        self.cand_rotation = 0;
        self.cand_swap = 0;
        let ch0 = self.s.mode45_channel0 as i32;
        for p in ch0..self.s.channels {
            self.mode45_candidate(&mut best, 4, p, 0);
            self.mode45_candidate(&mut best, 4, p, 1);
        }
        if best < self.best_err {
            self.best_err = best;
            self.code_mode45(4);
        }
        for p in ch0..self.s.channels {
            self.mode45_candidate(&mut best, 5, p, 0);
        }
        if best < self.best_err {
            self.best_err = best;
            self.code_mode45(5);
        }
        self.cur = self.blk;
    }

    fn enc_mode6(&mut self) {
        let channels = self.s.channels;
        self.block_segment(0, -1, channels);
        if channels == 3 {
            self.ep[3] = 255.0;
            self.ep[7] = 255.0;
        }
        self.ep_quant_dequant(6, channels);
        let mut err = self.block_quant(4, 0, channels);
        for _ in 0..self.refine(6) {
            self.opt_endpoints(0, 4, -1, channels);
            self.ep_quant_dequant(6, channels);
            err = self.block_quant(4, 0, channels);
        }
        if err < self.best_err {
            self.best_err = err;
            let mut q = [0i32; 8];
            q.copy_from_slice(&self.qep[..8]);
            let qb = self.qb;
            self.code_mode6(q, qb);
        }
    }
}

fn covar_from_stats(s: &[f32; 15], channels: i32) -> [f32; 10] {
    let mut c = [0f32; 10];
    c[0] = s[0] - (s[10] * s[10]) / s[14];
    c[1] = s[1] - (s[10] * s[11]) / s[14];
    c[2] = s[2] - (s[10] * s[12]) / s[14];
    c[4] = s[4] - (s[11] * s[11]) / s[14];
    c[5] = s[5] - (s[11] * s[12]) / s[14];
    c[7] = s[7] - (s[12] * s[12]) / s[14];
    if channels == 4 {
        c[3] = s[3] - (s[10] * s[13]) / s[14];
        c[6] = s[6] - (s[11] * s[13]) / s[14];
        c[8] = s[8] - (s[12] * s[13]) / s[14];
        c[9] = s[9] - (s[13] * s[13]) / s[14];
    }
    c
}

fn ssymv(c: &[f32; 10], b: &[f32; 4], channels: i32) -> [f32; 4] {
    let mut a = [0f32; 4];
    if channels == 3 {
        a[0] = fma(c[2], b[2], fma(c[0], b[0], c[1] * b[1]));
        a[1] = fma(c[5], b[2], fma(c[1], b[0], c[4] * b[1]));
        a[2] = fma(c[7], b[2], fma(c[2], b[0], c[5] * b[1]));
    } else {
        a[0] = fma(c[3], b[3], fma(c[2], b[2], fma(c[0], b[0], c[1] * b[1])));
        a[1] = fma(c[6], b[3], fma(c[5], b[2], fma(c[1], b[0], c[4] * b[1])));
        a[2] = fma(c[8], b[3], fma(c[7], b[2], fma(c[2], b[0], c[5] * b[1])));
        a[3] = fma(c[9], b[3], fma(c[8], b[2], fma(c[3], b[0], c[6] * b[1])));
    }
    a
}

fn compute_axis(c: &[f32; 10], iterations: i32, channels: i32) -> [f32; 4] {
    let mut vec = [1f32; 4];
    for i in 0..iterations {
        let axis = ssymv(c, &vec, channels);
        vec[..channels as usize].copy_from_slice(&axis[..channels as usize]);
        if i % 2 == 1 {
            let mut norm_sq = 0f32;
            for p in 0..channels as usize {
                norm_sq = fma(axis[p], axis[p], norm_sq);
            }
            let rnorm = 1.0 / norm_sq.sqrt();
            for p in 0..channels as usize {
                vec[p] *= rnorm;
            }
        }
    }
    vec
}

fn get_pca_bound(c_in: [f32; 10], channels: i32) -> f32 {
    let mut c = c_in;
    for k in 0..10 {
        c[k] *= 1.0 / 65536.0;
    }
    let eps = f32::from_bits(0x358637BD);
    c[0] += eps;
    c[4] += eps;
    c[7] += eps;
    let axis = compute_axis(&c, 4, channels);
    let v = ssymv(&c, &axis, channels);
    let mut sq_sum = 0f32;
    for p in 0..channels as usize {
        sq_sum = fma(v[p], v[p], sq_sum);
    }
    let lambda = sq_sum.sqrt();
    let mut bound = c[0] + c[4] + c[7];
    if channels == 4 {
        bound += c[9];
    }
    bound -= lambda;
    bound.max(0.0)
}

fn channel_quant_dequant(e: &mut [f32; 2], q: &mut [i32; 2], epbits: i32) {
    let elevels = 1 << epbits;
    for i in 0..2 {
        let v = cvt(fma(e[i] / 255.0, (elevels - 1) as f32, 0.5));
        q[i] = v.clamp(0, elevels - 1);
        e[i] = unpack_to_byte(q[i], epbits) as f32;
    }
}

fn swap456(q: &mut [i32; 8], channels: usize, qblock: &mut [u32; 2], bits: i32) {
    let levels = 1 << bits;
    if (qblock[0] & 15) as i32 >= levels / 2 {
        for p in 0..channels {
            q.swap(p, channels + p);
        }
        for k in 0..2 {
            qblock[k] = (0x11111111i32.wrapping_mul(levels - 1) as u32).wrapping_sub(qblock[k]);
        }
    }
}

/// One block (RGBA8 pixels, row-major 4x4) with the given ISPC settings: 16 bytes of BC7.
pub fn encode_block(px: &[[u8; 4]; 16], s: &EncodeSettings) -> [u8; 16] {
    let mut blk = [0f32; 64];
    for k in 0..16 {
        for c in 0..4 {
            blk[c * 16 + k] = px[k][c] as f32;
        }
    }
    let mut st = St { s: *s, blk, cur: blk, best_err: BIG, opaque_err: 0.0, best_data: [0; 5], ep: [0.0; 24], qep: [0; 24], qb: [0; 2], cand_qep: [0; 8], cand_qblock: [0; 2], cand_aqep: [0; 2], cand_aqblock: [0; 2], cand_rotation: 0, cand_swap: 0 };
    if s.channels == 4 {
        for k in 0..16 {
            let d = blk[48 + k] - 255.0;
            st.opaque_err = fma(d, d, st.opaque_err);
        }
    }
    if s.mode_selection[0] {
        let list: [i32; 64] = std::array::from_fn(|i| i as i32);
        st.enc_mode01237(0, &list, 16);
        if !s.skip_mode2 {
            st.enc_mode01237(2, &list, 64);
        }
    }
    if s.mode_selection[1] {
        let (fs1, fs3) = (s.fast_skip_threshold_mode1 as i32, s.fast_skip_threshold_mode3 as i32);
        if fs1 != 0 || fs3 != 0 {
            let mut list = st.bound_list(3);
            partial_sort(&mut list, fs1.max(fs3));
            st.enc_mode01237(1, &list, fs1);
            st.enc_mode01237(3, &list, fs3);
        }
        let fs7 = s.fast_skip_threshold_mode7 as i32;
        if fs7 != 0 {
            let mut list = st.bound_list(s.channels);
            partial_sort(&mut list, fs7);
            st.enc_mode01237(7, &list, fs7);
        }
    }
    if s.mode_selection[2] {
        st.enc_mode45();
    }
    if s.mode_selection[3] {
        st.enc_mode6();
    }
    let mut out = [0u8; 16];
    for k in 0..4 {
        out[k * 4..k * 4 + 4].copy_from_slice(&st.best_data[k].to_le_bytes());
    }
    out
}

fn partial_sort(list: &mut [i32; 64], count: i32) {
    for k in 0..count as usize {
        let (mut bi, mut bv) = (k, list[k]);
        for i in k + 1..64 {
            if bv > list[i] {
                bv = list[i];
                bi = i;
            }
        }
        list[bi] = list[k];
        list[k] = bv;
    }
}

/// The settings [crate::bc7_cpu] uses for an image.
pub fn settings(quality: crate::Bc7Quality, opaque: bool) -> EncodeSettings {
    crate::bc7_cpu::settings(quality, opaque)
}
