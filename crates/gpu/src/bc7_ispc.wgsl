// SPDX-License-Identifier: GPL-3.0-only AND MIT
// BC7 encoder: a WGSL port of the BC7 part of Intel's ISPC Texture Compressor (kernel.ispc,
// Copyright (c) 2016 Intel Corporation, MIT licence; see THIRD-PARTY-NOTICES.txt). One invocation
// encodes one 4x4 block with the same search as the CPU encoder `bc7_cpu` (intel_tex_2), so both
// write the same blocks. Where float results could differ, the port follows the x86 build:
// - divisions and square roots are corrected to IEEE (the kernel is built without fast-math),
// - multiply-adds are written as fma() where LLVM fuses them on the FMA targets (a single-use
//   multiply feeding an add/sub; the left operand first),
// - float -> int conversions out of range (or NaN) give INT_MIN like cvttps2dq.

struct Params {
    blocks_x: u32,
    blocks_y: u32,
    // Pixels per row of the (block-padded) source.
    stride: u32,
    channels: i32,
    // mode_selection[0..4]: mode 0/2, modes 1/3/7, modes 4/5, mode 6.
    mode_sel: vec4<i32>,
    refine_lo: vec4<i32>,
    refine_hi: vec4<i32>,
    skip_mode2: i32,
    fs1: i32,
    fs3: i32,
    fs7: i32,
    m45_channel0: i32,
    refine_channel: i32,
    groups_x: u32,
    _pad: u32,
    // (255, 16, 0, 0): divisors passed at run time, so the compiler cannot fold the exact-division
    // correction of fdiv() away for constant divisors (it does: 85/255 came out one ulp high).
    consts: vec4<f32>,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
// 0..128 partition patterns, 128..256 pattern masks, 256..384 anchor skips,
// 384..388 / 388..396 / 396..412 index weights for 2, 3 and 4 bits.
@group(0) @binding(3) var<storage, read> tables: array<u32>;

const BIG: f32 = 3.0e38; // stands for the kernel's 1e99 (+inf as a float)
const EPS2: u32 = 0x358637BDu; // bits of sq(0.001f) evaluated in f32

var<private> blk: array<f32, 64>;  // the block, planar: r[16] g[16] b[16] a[16]
var<private> cur: array<f32, 64>;  // the block the endpoint search reads (rotated for modes 4/5)
var<private> best_err: f32;
var<private> opaque_err: f32;
var<private> best_data: array<u32, 5>;

var<private> ep: array<f32, 24>;
var<private> qep: array<i32, 24>;
var<private> qb: array<u32, 2>;

// ---------------------------------------------------------------- helpers

fn cvt(x: f32) -> i32 {
    // (int)x on x86: truncation; |x| >= 2^31, inf and NaN give INT_MIN.
    if (((bitcast<u32>(x) >> 23u) & 0xffu) >= 158u) {
        return bitcast<i32>(0x80000000u);
    }
    return i32(x);
}

fn fdiv(a: f32, b: f32) -> f32 {
    // IEEE division from the hardware approximation plus one fma correction.
    if (b == 0.0) {
        return a / b;
    }
    let y = 1.0 / b;
    let q = a * y;
    let r = fma(-q, b, a);
    return fma(r, y, q);
}

fn fsqrt(x: f32) -> f32 {
    if (x <= 0.0) {
        return sqrt(x);
    }
    let s = sqrt(x);
    let r = fma(-s, s, x);
    return fma(r, 0.5 / s, s);
}

fn rsqrt_(x: f32) -> f32 {
    return fdiv(1.0, fsqrt(x));
}

fn sq(v: f32) -> f32 {
    return v * v;
}

fn refine(mode: i32) -> i32 {
    if (mode < 4) {
        return P.refine_lo[mode];
    }
    return P.refine_hi[mode - 4];
}

fn get_pattern(part_id: i32) -> u32 {
    return tables[part_id];
}

fn get_pattern_mask(part_id: i32, j: i32) -> i32 {
    let packed = tables[128 + part_id];
    let mask0 = i32(packed & 0xFFFFu);
    let mask1 = i32(packed >> 16u);
    if (j == 2) {
        return (~mask0) & (~mask1);
    }
    if (j == 0) {
        return mask0;
    }
    return mask1;
}

fn get_skip(part_id: i32, j: i32) -> i32 {
    let packed = i32(tables[256 + part_id]);
    if (j == 0) {
        return 0;
    }
    if (j == 1) {
        return packed >> 4u;
    }
    return packed & 15;
}

fn unquant(bits: i32, i: i32) -> i32 {
    var base = 384;
    if (bits == 3) {
        base = 388;
    }
    if (bits == 4) {
        base = 396;
    }
    return i32(tables[base + i]);
}

fn unpack_to_byte(v: i32, bits: i32) -> i32 {
    let vv = v << u32(8 - bits);
    return vv + (vv >> u32(bits));
}

// ---------------------------------------------------------------- PCA

// stats: 0..10 second moments, 10..14 sums, 14 count.
fn compute_stats_masked(mask: i32, channels: i32) -> array<f32, 15> {
    var s: array<f32, 15>;
    for (var k = 0; k < 16; k++) {
        let flag = f32((mask >> u32(k)) & 1);
        var c: array<f32, 4>;
        for (var p = 0; p < channels; p++) {
            c[p] = cur[k + p * 16] * flag;
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
        if (channels == 4) {
            s[13] += c[3];
            s[3] = fma(c[0], c[3], s[3]);
            s[6] = fma(c[1], c[3], s[6]);
            s[8] = fma(c[2], c[3], s[8]);
            s[9] = fma(c[3], c[3], s[9]);
        }
    }
    return s;
}

fn covar_from_stats(s: array<f32, 15>, channels: i32) -> array<f32, 10> {
    var c: array<f32, 10>;
    c[0] = s[0] - fdiv(s[10] * s[10], s[14]);
    c[1] = s[1] - fdiv(s[10] * s[11], s[14]);
    c[2] = s[2] - fdiv(s[10] * s[12], s[14]);
    c[4] = s[4] - fdiv(s[11] * s[11], s[14]);
    c[5] = s[5] - fdiv(s[11] * s[12], s[14]);
    c[7] = s[7] - fdiv(s[12] * s[12], s[14]);
    if (channels == 4) {
        c[3] = s[3] - fdiv(s[10] * s[13], s[14]);
        c[6] = s[6] - fdiv(s[11] * s[13], s[14]);
        c[8] = s[8] - fdiv(s[12] * s[13], s[14]);
        c[9] = s[9] - fdiv(s[13] * s[13], s[14]);
    }
    return c;
}

fn ssymv(c: array<f32, 10>, b: array<f32, 4>, channels: i32) -> array<f32, 4> {
    var a: array<f32, 4>;
    if (channels == 3) {
        a[0] = fma(c[2], b[2], fma(c[0], b[0], c[1] * b[1]));
        a[1] = fma(c[5], b[2], fma(c[1], b[0], c[4] * b[1]));
        a[2] = fma(c[7], b[2], fma(c[2], b[0], c[5] * b[1]));
    } else {
        a[0] = fma(c[3], b[3], fma(c[2], b[2], fma(c[0], b[0], c[1] * b[1])));
        a[1] = fma(c[6], b[3], fma(c[5], b[2], fma(c[1], b[0], c[4] * b[1])));
        a[2] = fma(c[8], b[3], fma(c[7], b[2], fma(c[2], b[0], c[5] * b[1])));
        a[3] = fma(c[9], b[3], fma(c[8], b[2], fma(c[3], b[0], c[6] * b[1])));
    }
    return a;
}

fn compute_axis(c: array<f32, 10>, iterations: i32, channels: i32) -> array<f32, 4> {
    var vec = array<f32, 4>(1.0, 1.0, 1.0, 1.0);
    var axis: array<f32, 4>;
    for (var i = 0; i < iterations; i++) {
        axis = ssymv(c, vec, channels);
        for (var p = 0; p < channels; p++) {
            vec[p] = axis[p];
        }
        if (i % 2 == 1) {
            var norm_sq = 0.0;
            for (var p = 0; p < channels; p++) {
                norm_sq = fma(axis[p], axis[p], norm_sq);
            }
            let rnorm = rsqrt_(norm_sq);
            for (var p = 0; p < channels; p++) {
                vec[p] *= rnorm;
            }
        }
    }
    return vec;
}

// Endpoints of the masked pixels along their principal axis: ep[base..base+8], clamped to 0..255.
fn block_segment(base: i32, mask: i32, channels: i32) {
    let s = compute_stats_masked(mask, channels);
    var c = covar_from_stats(s, channels);
    var dc: array<f32, 4>;
    for (var p = 0; p < channels; p++) {
        dc[p] = fdiv(s[10 + p], s[14]);
    }
    let inv_var = 1.0 / 65536.0;
    for (var k = 0; k < 10; k++) {
        c[k] *= inv_var;
    }
    let eps = bitcast<f32>(EPS2);
    c[0] += eps;
    c[4] += eps;
    c[7] += eps;
    c[9] += eps;
    let axis = compute_axis(c, 8, channels);

    var lo = BIG;
    var hi = -BIG;
    for (var k = 0; k < 16; k++) {
        if (((mask >> u32(k)) & 1) == 0) {
            continue;
        }
        var dot = 0.0;
        for (var p = 0; p < channels; p++) {
            dot = fma(axis[p], cur[16 * p + k] - dc[p], dot);
        }
        lo = min(lo, dot);
        hi = max(hi, dot);
    }
    if (hi - lo < 1.0) {
        lo -= 0.5;
        hi += 0.5;
    }
    for (var p = 0; p < channels; p++) {
        ep[base + p] = clamp(fma(lo, axis[p], dc[p]), 0.0, 255.0);
        ep[base + 4 + p] = clamp(fma(hi, axis[p], dc[p]), 0.0, 255.0);
    }
}

fn get_pca_bound(c_in: array<f32, 10>, channels: i32) -> f32 {
    var c = c_in;
    let inv_var = 1.0 / 65536.0;
    for (var k = 0; k < 10; k++) {
        c[k] *= inv_var;
    }
    let eps = bitcast<f32>(EPS2);
    c[0] += eps;
    c[4] += eps;
    c[7] += eps;
    let axis = compute_axis(c, 4, channels);
    let v = ssymv(c, axis, channels);
    var sq_sum = 0.0;
    for (var p = 0; p < channels; p++) {
        sq_sum = fma(v[p], v[p], sq_sum);
    }
    let lambda = fsqrt(sq_sum);
    var bound = c[0] + c[4] + c[7];
    if (channels == 4) {
        bound += c[9];
    }
    bound -= lambda;
    return max(bound, 0.0);
}

fn block_pca_bound_split(mask: i32, full: array<f32, 15>, channels: i32) -> f32 {
    var s = compute_stats_masked(mask, channels);
    let c1 = covar_from_stats(s, channels);
    for (var i = 0; i < 15; i++) {
        s[i] = full[i] - s[i];
    }
    let c2 = covar_from_stats(s, channels);
    var bound = 0.0;
    bound += get_pca_bound(c1, channels);
    bound += get_pca_bound(c2, channels);
    return fsqrt(bound) * 256.0;
}

// ---------------------------------------------------------------- endpoint quantization

fn ep_quant0367(base: i32, mode: i32, channels: i32) {
    var bits = 7;
    if (mode == 0) {
        bits = 4;
    }
    if (mode == 7) {
        bits = 5;
    }
    let levels = 1 << u32(bits);
    let levels2 = levels * 2 - 1;
    for (var i = 0; i < 2; i++) {
        var qb_: array<i32, 8>;
        for (var b = 0; b < 2; b++) {
            for (var p = 0; p < 4; p++) {
                let t = fma(fdiv(ep[base + i * 4 + p], P.consts.x), f32(levels2), -f32(b));
                let v = cvt(t * 0.5 + 0.5) * 2 + b;
                qb_[b * 4 + p] = clamp(v, b, levels2 - 1 + b);
            }
        }
        var e: array<f32, 8>;
        for (var j = 0; j < 8; j++) {
            e[j] = f32(qb_[j]);
            if (mode == 0) {
                e[j] = f32(unpack_to_byte(qb_[j], 5));
            }
        }
        var err0 = 0.0;
        var err1 = 0.0;
        for (var p = 0; p < channels; p++) {
            let d0 = ep[base + i * 4 + p] - e[p];
            let d1 = ep[base + i * 4 + p] - e[4 + p];
            err0 = fma(d0, d0, err0);
            err1 = fma(d1, d1, err1);
        }
        for (var p = 0; p < 4; p++) {
            if (err0 < err1) {
                qep[base + i * 4 + p] = qb_[p];
            } else {
                qep[base + i * 4 + p] = qb_[4 + p];
            }
        }
    }
}

fn ep_quant1(base: i32) {
    var qb_: array<i32, 16>;
    for (var b = 0; b < 2; b++) {
        for (var i = 0; i < 8; i++) {
            let t = fma(fdiv(ep[base + i], P.consts.x), 127.0, -f32(b));
            let v = cvt(t * 0.5 + 0.5) * 2 + b;
            qb_[b * 8 + i] = clamp(v, b, 126 + b);
        }
    }
    var e: array<f32, 16>;
    for (var k = 0; k < 16; k++) {
        e[k] = f32(unpack_to_byte(qb_[k], 7));
    }
    var err0 = 0.0;
    var err1 = 0.0;
    for (var j = 0; j < 2; j++) {
        for (var p = 0; p < 3; p++) {
            let d0 = ep[base + j * 4 + p] - e[j * 4 + p];
            let d1 = ep[base + j * 4 + p] - e[8 + j * 4 + p];
            err0 = fma(d0, d0, err0);
            err1 = fma(d1, d1, err1);
        }
    }
    for (var i = 0; i < 8; i++) {
        if (err0 < err1) {
            qep[base + i] = qb_[i];
        } else {
            qep[base + i] = qb_[8 + i];
        }
    }
}

fn ep_quant245(base: i32, mode: i32) {
    var bits = 5;
    if (mode == 5) {
        bits = 7;
    }
    let levels = 1 << u32(bits);
    for (var i = 0; i < 8; i++) {
        let v = cvt(fma(fdiv(ep[base + i], P.consts.x), f32(levels - 1), 0.5));
        qep[base + i] = clamp(v, 0, levels - 1);
    }
}

fn pairs_of(mode: i32) -> i32 {
    if (mode == 0 || mode == 2) {
        return 3;
    }
    if (mode == 4 || mode == 5 || mode == 6) {
        return 1;
    }
    return 2;
}

fn ep_quant_dequant(mode: i32, channels: i32) {
    let pairs = pairs_of(mode);
    for (var i = 0; i < pairs; i++) {
        if (mode == 0 || mode == 3 || mode == 6 || mode == 7) {
            ep_quant0367(i * 8, mode, channels);
        } else if (mode == 1) {
            ep_quant1(i * 8);
        } else {
            ep_quant245(i * 8, mode);
        }
    }
    for (var i = 0; i < 8 * pairs; i++) {
        if (mode == 3 || mode == 6) {
            ep[i] = f32(qep[i]);
        } else if (mode == 1 || mode == 5) {
            ep[i] = f32(unpack_to_byte(qep[i], 7));
        } else if (mode == 7) {
            ep[i] = f32(unpack_to_byte(qep[i], 6));
        } else {
            ep[i] = f32(unpack_to_byte(qep[i], 5));
        }
    }
}

// ---------------------------------------------------------------- pixel quantization

fn block_quant(bits: i32, pattern: u32, channels: i32) -> f32 {
    var total = 0.0;
    let levels = 1 << u32(bits);
    qb[0] = 0u;
    qb[1] = 0u;
    var ps = pattern;
    for (var k = 0; k < 16; k++) {
        let j = i32(ps & 3u);
        ps >>= 2u;
        var proj = 0.0;
        var div = 0.0;
        for (var p = 0; p < channels; p++) {
            let a = ep[8 * j + p];
            let b = ep[8 * j + 4 + p];
            proj = fma(cur[k + p * 16] - a, b - a, proj);
            div = fma(b - a, b - a, div);
        }
        proj = fdiv(proj, div);
        let q1 = clamp(cvt(fma(proj, f32(levels), 0.5)), 1, levels - 1);
        let w0 = unquant(bits, q1 - 1);
        let w1 = unquant(bits, q1);
        var err0 = 0.0;
        var err1 = 0.0;
        for (var p = 0; p < channels; p++) {
            let a = ep[8 * j + p];
            let b = ep[8 * j + 4 + p];
            let d0 = f32(cvt((fma(f32(64 - w0), a, f32(w0) * b) + 32.0) * 0.015625)) - cur[k + p * 16];
            let d1 = f32(cvt((fma(f32(64 - w1), a, f32(w1) * b) + 32.0) * 0.015625)) - cur[k + p * 16];
            err0 = fma(d0, d0, err0);
            err1 = fma(d1, d1, err1);
        }
        var be = cvt(err1);
        var bq = q1;
        if (err0 < err1) {
            be = cvt(err0);
            bq = q1 - 1;
        }
        qb[k / 8] += u32(bq) << u32(4 * (k % 8));
        total += f32(be);
    }
    return total;
}

// Least-squares endpoints for the indices in qb: ep[base..base+8].
fn opt_endpoints(base: i32, bits: i32, mask: i32, channels: i32) {
    let levels = 1 << u32(bits);
    var atb1 = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var sum_q = 0.0;
    var sum_qq = 0.0;
    var sum = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
    for (var k1 = 0; k1 < 2; k1++) {
        var qs = qb[k1];
        for (var k2 = 0; k2 < 8; k2++) {
            let k = k1 * 8 + k2;
            let q = f32(i32(qs & 15u));
            qs >>= 4u;
            if (((mask >> u32(k)) & 1) == 0) {
                continue;
            }
            let x = cvt(f32(levels - 1) - q);
            sum_q += q;
            sum_qq = fma(q, q, sum_qq);
            sum[4] += 1.0;
            for (var p = 0; p < channels; p++) {
                sum[p] += cur[k + p * 16];
            }
            for (var p = 0; p < channels; p++) {
                atb1[p] = fma(f32(x), cur[k + p * 16], atb1[p]);
            }
        }
    }
    var atb2: array<f32, 4>;
    for (var p = 0; p < channels; p++) {
        atb2[p] = fma(f32(levels - 1), sum[p], -atb1[p]);
    }
    let l1 = f32(levels - 1);
    let cxx = fma(sum[4], sq(l1), -(f32(2 * (levels - 1)) * sum_q)) + sum_qq;
    let cyy = sum_qq;
    let cxy = fma(l1, sum_q, -sum_qq);
    let det = fma(cxx, cyy, -(cxy * cxy));
    let scale = fdiv(l1, det);
    for (var p = 0; p < channels; p++) {
        ep[base + p] = fma(atb1[p], cyy, -(atb2[p] * cxy)) * scale;
        ep[base + 4 + p] = fma(atb2[p], cxx, -(atb1[p] * cxy)) * scale;
    }
    if (abs(det) < 0.001) {
        for (var p = 0; p < channels; p++) {
            ep[base + p] = fdiv(sum[p], sum[4]);
            ep[base + 4 + p] = ep[base + p];
        }
    }
}

// ---------------------------------------------------------------- bitstream

fn put_bits(pos: ptr<function, i32>, bits: i32, v: u32) {
    let p = *pos;
    best_data[p / 32] |= v << u32(p % 32);
    if (p % 32 + bits > 32) {
        best_data[p / 32 + 1] |= v >> u32(32 - p % 32);
    }
    *pos = p + bits;
}

fn data_shl_1bit_from(at: i32) {
    if (at < 96) {
        let shifted = (best_data[2] >> 1u) | (best_data[3] << 31u);
        let mask = ((1u << u32(at - 64)) - 1u) >> 1u;
        best_data[2] = (mask & best_data[2]) | (~mask & shifted);
        best_data[3] = (best_data[3] >> 1u) | (best_data[4] << 31u);
        best_data[4] = best_data[4] >> 1u;
    } else if (at < 128) {
        let shifted = (best_data[3] >> 1u) | (best_data[4] << 31u);
        let mask = ((1u << u32(at - 96)) - 1u) >> 1u;
        best_data[3] = (mask & best_data[3]) | (~mask & shifted);
        best_data[4] = best_data[4] >> 1u;
    }
}

fn code_qblock(pos: ptr<function, i32>, q0: u32, q1: u32, bits: i32, flips: i32) {
    let levels = 1 << u32(bits);
    var fl = flips;
    for (var k1 = 0; k1 < 2; k1++) {
        var qs = q0;
        if (k1 == 1) {
            qs = q1;
        }
        for (var k2 = 0; k2 < 8; k2++) {
            var q = i32(qs & 15u);
            if ((fl & 1) > 0) {
                q = (levels - 1) - q;
            }
            if (k1 == 0 && k2 == 0) {
                put_bits(pos, bits - 1, u32(q));
            } else {
                put_bits(pos, bits, u32(q));
            }
            qs >>= 4u;
            fl >>= 1u;
        }
    }
}

fn code_mode01237(q_in: array<i32, 24>, qblock: array<u32, 2>, part_id: i32, mode: i32) {
    var bits = 2;
    if (mode == 0 || mode == 1) {
        bits = 3;
    }
    let pairs = pairs_of(mode);
    var channels = 3;
    if (mode == 7) {
        channels = 4;
    }
    var q = q_in;
    // Anchor indices must have their top bit clear: swap endpoints and flip indices.
    var flips = 0;
    let levels = 1 << u32(bits);
    for (var j = 0; j < pairs; j++) {
        let k0 = get_skip(part_id, j);
        let qv = i32((qblock[k0 >> 3u] << u32(28 - (k0 & 7) * 4)) >> 28u);
        if (qv >= levels / 2) {
            for (var p = 0; p < 4; p++) {
                let t = q[8 * j + p];
                q[8 * j + p] = q[8 * j + 4 + p];
                q[8 * j + 4 + p] = t;
            }
            flips |= get_pattern_mask(part_id, j);
        }
    }
    for (var k = 0; k < 5; k++) {
        best_data[k] = 0u;
    }
    var pos = 0;
    put_bits(&pos, mode + 1, 1u << u32(mode));
    if (mode == 0) {
        put_bits(&pos, 4, u32(part_id & 15));
    } else {
        put_bits(&pos, 6, u32(part_id & 63));
    }
    for (var p = 0; p < channels; p++) {
        for (var j = 0; j < pairs * 2; j++) {
            let v = q[j * 4 + p];
            if (mode == 0) {
                put_bits(&pos, 4, u32(v >> 1u));
            } else if (mode == 1) {
                put_bits(&pos, 6, u32(v >> 1u));
            } else if (mode == 2) {
                put_bits(&pos, 5, u32(v));
            } else if (mode == 3) {
                put_bits(&pos, 7, u32(v >> 1u));
            } else {
                put_bits(&pos, 5, u32(v >> 1u));
            }
        }
    }
    if (mode == 1) {
        for (var j = 0; j < 2; j++) {
            put_bits(&pos, 1, u32(q[j * 8] & 1));
        }
    }
    if (mode == 0 || mode == 3 || mode == 7) {
        for (var j = 0; j < pairs * 2; j++) {
            put_bits(&pos, 1, u32(q[j * 4] & 1));
        }
    }
    code_qblock(&pos, qblock[0], qblock[1], bits, flips);
    // Drop the anchor bits that were written as full indices.
    var s1 = get_skip(part_id, 1);
    var s2 = get_skip(part_id, 2);
    if (pairs > 2 && s1 < s2) {
        let t = s1;
        s1 = s2;
        s2 = t;
    }
    data_shl_1bit_from(128 + (pairs - 1) - (15 - s1) * bits);
    if (pairs > 2) {
        data_shl_1bit_from(128 + (pairs - 1) - (15 - s2) * bits);
    }
}

// Mode 4/5/6 swap: the first index must have its top bit clear.
fn swap456(q: ptr<function, array<i32, 8>>, channels: i32, qblock: ptr<function, array<u32, 2>>, bits: i32) {
    let levels = 1 << u32(bits);
    if (i32((*qblock)[0] & 15u) >= levels / 2) {
        for (var p = 0; p < channels; p++) {
            let t = (*q)[p];
            (*q)[p] = (*q)[channels + p];
            (*q)[channels + p] = t;
        }
        for (var k = 0; k < 2; k++) {
            (*qblock)[k] = u32(0x11111111 * (levels - 1)) - (*qblock)[k];
        }
    }
}

fn code_mode6(q_in: array<i32, 8>, qblock_in: array<u32, 2>) {
    var q = q_in;
    var qblock = qblock_in;
    swap456(&q, 4, &qblock, 4);
    for (var k = 0; k < 5; k++) {
        best_data[k] = 0u;
    }
    var pos = 0;
    put_bits(&pos, 7, 64u);
    for (var p = 0; p < 4; p++) {
        put_bits(&pos, 7, u32(q[p] >> 1u));
        put_bits(&pos, 7, u32(q[4 + p] >> 1u));
    }
    put_bits(&pos, 1, u32(q[0] & 1));
    put_bits(&pos, 1, u32(q[4] & 1));
    code_qblock(&pos, qblock[0], qblock[1], 4, 0);
}

// ---------------------------------------------------------------- modes 0-3, 7

fn bc7_enc_mode01237(mode: i32, part_list: ptr<function, array<i32, 64>>, part_count: i32) {
    if (part_count == 0) {
        return;
    }
    var bits = 2;
    if (mode == 0 || mode == 1) {
        bits = 3;
    }
    let pairs = pairs_of(mode);
    var channels = 3;
    if (mode == 7) {
        channels = 4;
    }
    var best_qep: array<i32, 24>;
    var best_qblock: array<u32, 2>;
    var best_part = -1;
    var best = BIG;
    for (var part = 0; part < part_count; part++) {
        var part_id = (*part_list)[part] & 63;
        if (pairs == 3) {
            part_id += 64;
        }
        for (var j = 0; j < pairs; j++) {
            block_segment(j * 8, get_pattern_mask(part_id, j), channels);
        }
        ep_quant_dequant(mode, channels);
        let err = block_quant(bits, get_pattern(part_id), channels);
        if (err < best) {
            for (var i = 0; i < 8 * pairs; i++) {
                best_qep[i] = qep[i];
            }
            best_qblock[0] = qb[0];
            best_qblock[1] = qb[1];
            best_part = part_id;
            best = err;
        }
    }
    let iterations = refine(mode);
    for (var it = 0; it < iterations; it++) {
        qb[0] = best_qblock[0];
        qb[1] = best_qblock[1];
        for (var j = 0; j < pairs; j++) {
            opt_endpoints(j * 8, bits, get_pattern_mask(best_part, j), channels);
        }
        ep_quant_dequant(mode, channels);
        let err = block_quant(bits, get_pattern(best_part), channels);
        if (err < best) {
            for (var i = 0; i < 8 * pairs; i++) {
                best_qep[i] = qep[i];
            }
            best_qblock[0] = qb[0];
            best_qblock[1] = qb[1];
            best = err;
        }
    }
    if (mode != 7) {
        best += opaque_err;
    }
    if (best < best_err) {
        best_err = best;
        code_mode01237(best_qep, best_qblock, best_part, mode);
    }
}

fn partial_sort_list(list: ptr<function, array<i32, 64>>, partial_count: i32) {
    for (var k = 0; k < partial_count; k++) {
        var best_idx = k;
        var best_value = (*list)[k];
        for (var i = k + 1; i < 64; i++) {
            if (best_value > (*list)[i]) {
                best_value = (*list)[i];
                best_idx = i;
            }
        }
        (*list)[best_idx] = (*list)[k];
        (*list)[k] = best_value;
    }
}

fn bc7_enc_mode02() {
    var list: array<i32, 64>;
    for (var i = 0; i < 64; i++) {
        list[i] = i;
    }
    bc7_enc_mode01237(0, &list, 16);
    if (P.skip_mode2 == 0) {
        bc7_enc_mode01237(2, &list, 64);
    }
}

fn bound_list(channels: i32) -> array<i32, 64> {
    let full = compute_stats_masked(-1, channels);
    var list: array<i32, 64>;
    for (var part = 0; part < 64; part++) {
        let bound = cvt(block_pca_bound_split(get_pattern_mask(part, 0), full, channels));
        list[part] = part + bound * 64;
    }
    return list;
}

fn bc7_enc_mode13() {
    if (P.fs1 == 0 && P.fs3 == 0) {
        return;
    }
    var list = bound_list(3);
    partial_sort_list(&list, max(P.fs1, P.fs3));
    bc7_enc_mode01237(1, &list, P.fs1);
    bc7_enc_mode01237(3, &list, P.fs3);
}

fn bc7_enc_mode7() {
    if (P.fs7 == 0) {
        return;
    }
    var list = bound_list(P.channels);
    partial_sort_list(&list, P.fs7);
    bc7_enc_mode01237(7, &list, P.fs7);
}

// ---------------------------------------------------------------- modes 4, 5

fn channel_quant_dequant(e: ptr<function, array<f32, 2>>, q: ptr<function, array<i32, 2>>, epbits: i32) {
    let elevels = 1 << u32(epbits);
    for (var i = 0; i < 2; i++) {
        let v = cvt(fma(fdiv((*e)[i], P.consts.x), f32(elevels - 1), 0.5));
        (*q)[i] = clamp(v, 0, elevels - 1);
        (*e)[i] = f32(unpack_to_byte((*q)[i], epbits));
    }
}

fn channel_opt_endpoints(e: ptr<function, array<f32, 2>>, ch: i32, bits: i32, q0: u32, q1: u32) {
    let levels = 1 << u32(bits);
    var atb1 = 0.0;
    var sum_q = 0.0;
    var sum_qq = 0.0;
    var sum = 0.0;
    for (var k1 = 0; k1 < 2; k1++) {
        var qs = q0;
        if (k1 == 1) {
            qs = q1;
        }
        for (var k2 = 0; k2 < 8; k2++) {
            let k = k1 * 8 + k2;
            let q = f32(i32(qs & 15u));
            qs >>= 4u;
            let x = cvt(f32(levels - 1) - q);
            sum_q += q;
            sum_qq = fma(q, q, sum_qq);
            sum += blk[ch * 16 + k];
            atb1 = fma(f32(x), blk[ch * 16 + k], atb1);
        }
    }
    let l1 = f32(levels - 1);
    let atb2 = fma(l1, sum, -atb1);
    let cxx = fma(16.0, sq(l1), -(f32(2 * (levels - 1)) * sum_q)) + sum_qq;
    let cyy = sum_qq;
    let cxy = fma(l1, sum_q, -sum_qq);
    let det = fma(cxx, cyy, -(cxy * cxy));
    let scale = fdiv(l1, det);
    (*e)[0] = fma(atb1, cyy, -(atb2 * cxy)) * scale;
    (*e)[1] = fma(atb2, cxx, -(atb1 * cxy)) * scale;
    (*e)[0] = clamp((*e)[0], 0.0, 255.0);
    (*e)[1] = clamp((*e)[1], 0.0, 255.0);
    if (abs(det) < 0.001) {
        (*e)[0] = fdiv(sum, P.consts.y);
        (*e)[1] = (*e)[0];
    }
}

fn channel_opt_quant(qblock: ptr<function, array<u32, 2>>, ch: i32, bits: i32, e: array<f32, 2>) -> f32 {
    let levels = 1 << u32(bits);
    (*qblock)[0] = 0u;
    (*qblock)[1] = 0u;
    var total = 0.0;
    for (var k = 0; k < 16; k++) {
        let v = blk[ch * 16 + k];
        let proj = fdiv(v - e[0], (e[1] - e[0]) + 0.001);
        let q1 = clamp(cvt(fma(proj, f32(levels), 0.5)), 1, levels - 1);
        let w0 = unquant(bits, q1 - 1);
        let w1 = unquant(bits, q1);
        let d0 = f32(cvt((fma(f32(64 - w0), e[0], f32(w0) * e[1]) + 32.0) * 0.015625)) - v;
        let d1 = f32(cvt((fma(f32(64 - w1), e[0], f32(w1) * e[1]) + 32.0) * 0.015625)) - v;
        let err0 = d0 * d0;
        let err1 = d1 * d1;
        var be = cvt(err1);
        var bq = q1;
        if (err0 < err1) {
            be = cvt(err0);
            bq = q1 - 1;
        }
        (*qblock)[k / 8] += u32(bq) << u32(4 * (k % 8));
        total += f32(be);
    }
    return total;
}

fn opt_channel(qblock: ptr<function, array<u32, 2>>, q: ptr<function, array<i32, 2>>, ch: i32, bits: i32, epbits: i32) -> f32 {
    var e = array<f32, 2>(255.0, 0.0);
    for (var k = 0; k < 16; k++) {
        e[0] = min(e[0], blk[ch * 16 + k]);
        e[1] = max(e[1], blk[ch * 16 + k]);
    }
    channel_quant_dequant(&e, q, epbits);
    var err = channel_opt_quant(qblock, ch, bits, e);
    for (var i = 0; i < P.refine_channel; i++) {
        channel_opt_endpoints(&e, ch, bits, (*qblock)[0], (*qblock)[1]);
        channel_quant_dequant(&e, q, epbits);
        err = channel_opt_quant(qblock, ch, bits, e);
    }
    return err;
}

var<private> cand_qep: array<i32, 8>;
var<private> cand_qblock: array<u32, 2>;
var<private> cand_aqep: array<i32, 2>;
var<private> cand_aqblock: array<u32, 2>;
var<private> cand_rotation: i32;
var<private> cand_swap: i32;

fn bc7_enc_mode45_candidate(best: ptr<function, f32>, mode: i32, rotation: i32, swap: i32) {
    var bits = 2;
    var abits = 2;
    if (mode == 4) {
        abits = 3;
    }
    var aepbits = 8;
    if (mode == 4) {
        aepbits = 6;
    }
    if (swap == 1) {
        bits = 3;
        abits = 2;
    }
    for (var k = 0; k < 16; k++) {
        for (var p = 0; p < 3; p++) {
            cur[k + p * 16] = blk[k + p * 16];
        }
        if (rotation < 3) {
            if (P.channels == 4) {
                cur[k + rotation * 16] = blk[k + 48];
            }
            if (P.channels == 3) {
                cur[k + rotation * 16] = 255.0;
            }
        }
    }
    block_segment(0, -1, 3);
    ep_quant_dequant(mode, 3);
    var err = block_quant(bits, 0u, 3);
    let iterations = refine(mode);
    for (var i = 0; i < iterations; i++) {
        opt_endpoints(0, bits, -1, 3);
        ep_quant_dequant(mode, 3);
        err = block_quant(bits, 0u, 3);
    }
    var aqep: array<i32, 2>;
    var aqblock: array<u32, 2>;
    err += opt_channel(&aqblock, &aqep, rotation, abits, aepbits);
    if (err < *best) {
        for (var i = 0; i < 8; i++) {
            cand_qep[i] = qep[i];
        }
        cand_qblock[0] = qb[0];
        cand_qblock[1] = qb[1];
        cand_aqep = aqep;
        cand_aqblock = aqblock;
        cand_rotation = rotation;
        cand_swap = swap;
        *best = err;
    }
}

fn code_mode45(mode: i32) {
    var q = cand_qep;
    var qblock = cand_qblock;
    var aq = array<i32, 8>(cand_aqep[0], cand_aqep[1], 0, 0, 0, 0, 0, 0);
    var aqblock = cand_aqblock;
    let bits = 2;
    var abits = 2;
    if (mode == 4) {
        abits = 3;
    }
    var epbits = 7;
    if (mode == 4) {
        epbits = 5;
    }
    var aepbits = 8;
    if (mode == 4) {
        aepbits = 6;
    }
    if (cand_swap == 0) {
        swap456(&q, 4, &qblock, bits);
        swap456(&aq, 1, &aqblock, abits);
    } else {
        let t = qblock;
        qblock = aqblock;
        aqblock = t;
        swap456(&aq, 1, &qblock, bits);
        swap456(&q, 4, &aqblock, abits);
    }
    for (var k = 0; k < 5; k++) {
        best_data[k] = 0u;
    }
    var pos = 0;
    put_bits(&pos, mode + 1, 1u << u32(mode));
    put_bits(&pos, 2, u32((cand_rotation + 1) & 3));
    if (mode == 4) {
        put_bits(&pos, 1, u32(cand_swap));
    }
    for (var p = 0; p < 3; p++) {
        put_bits(&pos, epbits, u32(q[p]));
        put_bits(&pos, epbits, u32(q[4 + p]));
    }
    put_bits(&pos, aepbits, u32(aq[0]));
    put_bits(&pos, aepbits, u32(aq[1]));
    code_qblock(&pos, qblock[0], qblock[1], bits, 0);
    code_qblock(&pos, aqblock[0], aqblock[1], abits, 0);
}

fn bc7_enc_mode45() {
    var best = best_err;
    cand_qep = array<i32, 8>();
    cand_qblock = array<u32, 2>();
    cand_aqep = array<i32, 2>();
    cand_aqblock = array<u32, 2>();
    cand_rotation = 0;
    cand_swap = 0;
    for (var p = P.m45_channel0; p < P.channels; p++) {
        bc7_enc_mode45_candidate(&best, 4, p, 0);
        bc7_enc_mode45_candidate(&best, 4, p, 1);
    }
    if (best < best_err) {
        best_err = best;
        code_mode45(4);
    }
    for (var p = P.m45_channel0; p < P.channels; p++) {
        bc7_enc_mode45_candidate(&best, 5, p, 0);
    }
    if (best < best_err) {
        best_err = best;
        code_mode45(5);
    }
    cur = blk;
}

// ---------------------------------------------------------------- mode 6

fn bc7_enc_mode6() {
    let channels = P.channels;
    block_segment(0, -1, channels);
    if (channels == 3) {
        ep[3] = 255.0;
        ep[7] = 255.0;
    }
    ep_quant_dequant(6, channels);
    var err = block_quant(4, 0u, channels);
    for (var i = 0; i < refine(6); i++) {
        opt_endpoints(0, 4, -1, channels);
        ep_quant_dequant(6, channels);
        err = block_quant(4, 0u, channels);
    }
    if (err < best_err) {
        best_err = err;
        var q: array<i32, 8>;
        for (var i = 0; i < 8; i++) {
            q[i] = qep[i];
        }
        code_mode6(q, array<u32, 2>(qb[0], qb[1]));
    }
}

// ---------------------------------------------------------------- entry

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let id = (wg.y * P.groups_x + wg.x) * 64u + li;
    if (id >= P.blocks_x * P.blocks_y) {
        return;
    }
    let bx = id % P.blocks_x;
    let by = id / P.blocks_x;
    for (var y = 0u; y < 4u; y++) {
        for (var x = 0u; x < 4u; x++) {
            let c = src[(by * 4u + y) * P.stride + bx * 4u + x];
            let k = y * 4u + x;
            blk[k] = f32(c & 255u);
            blk[16u + k] = f32((c >> 8u) & 255u);
            blk[32u + k] = f32((c >> 16u) & 255u);
            blk[48u + k] = f32(c >> 24u);
        }
    }
    cur = blk;
    best_err = BIG;
    opaque_err = 0.0;
    if (P.channels == 4) {
        for (var k = 0; k < 16; k++) {
            let d = blk[48 + k] - 255.0;
            opaque_err = fma(d, d, opaque_err);
        }
    }
    for (var k = 0; k < 5; k++) {
        best_data[k] = 0u;
    }
    if (P.mode_sel[0] != 0) {
        bc7_enc_mode02();
    }
    if (P.mode_sel[1] != 0) {
        bc7_enc_mode13();
        bc7_enc_mode7();
    }
    if (P.mode_sel[2] != 0) {
        bc7_enc_mode45();
    }
    if (P.mode_sel[3] != 0) {
        bc7_enc_mode6();
    }
    for (var k = 0u; k < 4u; k++) {
        dst[id * 4u + k] = best_data[k];
    }
}
