// BC7 mode 6 encoder: one thread per 4x4 block.
//
// Mode 6 block (128 bits, LSB first): mode 7 bits (0b1000000), R0 R1 G0 G1 B0 B1 A0 A1 (7 bits
// each), P0, P1 (1 bit each), 16 colour indices of 4 bits; the first (anchor) index is stored in
// 3 bits, so its top bit must be 0 (endpoints are swapped when it would be 1).
// Endpoint value = (q7 << 1) | p. Palette entry i = ((64 - w[i]) * e0 + w[i] * e1 + 32) >> 6.
//
// Encoding: principal axis by power iteration on the block covariance, endpoints from the
// projected extent, then least-squares endpoint refinement; the best of the candidates
// (by squared RGBA error) is written.

struct Params {
    width: u32,        // pixels
    height: u32,       // pixels
    blocks_x: u32,
    blocks_y: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> pixels: array<u32>;      // RGBA8, R in the low byte
@group(0) @binding(2) var<storage, read_write> blocks: array<vec4<u32>>;

const WEIGHTS = array<i32, 16>(0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64);

var<private> px: array<vec4<f32>, 16>;

fn unpack(c: u32) -> vec4<f32> {
    return vec4<f32>(f32(c & 0xffu), f32((c >> 8u) & 0xffu), f32((c >> 16u) & 0xffu), f32(c >> 24u));
}

// Quantize an endpoint to 7 bits + shared p bit; returns the 8-bit reconstruction in .xyzw
// and writes the 7-bit values and p bit through pointers.
fn quantize(e: vec4<f32>, q: ptr<function, vec4<u32>>, p: ptr<function, u32>) -> vec4<f32> {
    var best_err = 1e30;
    var best = vec4<f32>(0.0);
    for (var pb = 0u; pb < 2u; pb++) {
        let qv = clamp(round((e - f32(pb)) * 0.5), vec4<f32>(0.0), vec4<f32>(127.0));
        let r = qv * 2.0 + f32(pb);
        let d = r - e;
        let err = dot(d, d);
        if (err < best_err) {
            best_err = err;
            best = r;
            *q = vec4<u32>(qv);
            *p = pb;
        }
    }
    return best;
}

fn palette(e0: vec4<f32>, e1: vec4<f32>, i: u32) -> vec4<f32> {
    let w = f32(WEIGHTS[i]);
    return floor(((64.0 - w) * e0 + w * e1 + 32.0) / 64.0);
}

struct Fit {
    q0: vec4<u32>,
    q1: vec4<u32>,
    p0: u32,
    p1: u32,
    idx: array<u32, 16>,
    err: f32,
};

// Quantizes the endpoints, assigns each pixel its best palette index and measures the error.
fn evaluate(a: vec4<f32>, b: vec4<f32>) -> Fit {
    var f: Fit;
    var q0: vec4<u32>; var q1: vec4<u32>; var p0: u32; var p1: u32;
    let e0 = quantize(a, &q0, &p0);
    let e1 = quantize(b, &q1, &p1);
    var pal: array<vec4<f32>, 16>;
    for (var i = 0u; i < 16u; i++) { pal[i] = palette(e0, e1, i); }
    var total = 0.0;
    for (var k = 0u; k < 16u; k++) {
        var best = 1e30;
        var bi = 0u;
        for (var i = 0u; i < 16u; i++) {
            let d = pal[i] - px[k];
            let e = dot(d, d);
            if (e < best) { best = e; bi = i; }
        }
        f.idx[k] = bi;
        total += best;
    }
    f.q0 = q0; f.q1 = q1; f.p0 = p0; f.p1 = p1; f.err = total;
    return f;
}

// Least-squares endpoints for fixed indices (weights t = w/64).
fn refine(f: Fit, a: vec4<f32>, b: vec4<f32>, ra: ptr<function, vec4<f32>>, rb: ptr<function, vec4<f32>>) {
    var aa = 0.0; var ab = 0.0; var bb = 0.0;
    var ax = vec4<f32>(0.0); var bx = vec4<f32>(0.0);
    for (var k = 0u; k < 16u; k++) {
        let t = f32(WEIGHTS[f.idx[k]]) / 64.0;
        let s = 1.0 - t;
        aa += s * s; ab += s * t; bb += t * t;
        ax += s * px[k]; bx += t * px[k];
    }
    let det = aa * bb - ab * ab;
    if (abs(det) < 1e-6) { *ra = a; *rb = b; return; }
    *ra = clamp((ax * bb - bx * ab) / det, vec4<f32>(0.0), vec4<f32>(255.0));
    *rb = clamp((bx * aa - ax * ab) / det, vec4<f32>(0.0), vec4<f32>(255.0));
}

var<private> out_bits: array<u32, 4>;

fn put(pos: u32, n: u32, v: u32) {
    // n <= 7 here, so a value spans at most two words.
    let w = pos >> 5u;
    let o = pos & 31u;
    out_bits[w] |= v << o;
    if (o + n > 32u) { out_bits[w + 1u] |= v >> (32u - o); }
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    // 2D dispatch: rows of `groups.x` workgroups (one dimension is capped at 65535 groups).
    let bi = gid.y * groups.x * 64u + gid.x;
    if (bi >= params.blocks_x * params.blocks_y) { return; }
    let bx = bi % params.blocks_x;
    let by = bi / params.blocks_x;

    var mean = vec4<f32>(0.0);
    for (var k = 0u; k < 16u; k++) {
        let x = min(bx * 4u + (k & 3u), params.width - 1u);
        let y = min(by * 4u + (k >> 2u), params.height - 1u);
        px[k] = unpack(pixels[y * params.width + x]);
        mean += px[k];
    }
    mean /= 16.0;

    // Covariance and principal axis.
    var cov = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    for (var k = 0u; k < 16u; k++) {
        let d = px[k] - mean;
        cov += mat4x4<f32>(d * d.x, d * d.y, d * d.z, d * d.w);
    }
    var axis = vec4<f32>(1.0, 1.0, 1.0, 0.25);
    for (var it = 0; it < 8; it++) {
        let n = cov * axis;
        let l = length(n);
        if (l < 1e-6) { break; }
        axis = n / l;
    }
    var tmin = 1e30; var tmax = -1e30;
    for (var k = 0u; k < 16u; k++) {
        let t = dot(px[k] - mean, axis);
        tmin = min(tmin, t); tmax = max(tmax, t);
    }
    var a = clamp(mean + axis * tmin, vec4<f32>(0.0), vec4<f32>(255.0));
    var b = clamp(mean + axis * tmax, vec4<f32>(0.0), vec4<f32>(255.0));

    var best = evaluate(a, b);
    for (var it = 0; it < 2; it++) {
        var ra: vec4<f32>; var rb: vec4<f32>;
        refine(best, a, b, &ra, &rb);
        let f = evaluate(ra, rb);
        if (f.err < best.err) { best = f; a = ra; b = rb; } else { break; }
    }

    // Anchor index top bit must be 0: swap endpoints and invert indices if needed.
    var q0 = best.q0; var q1 = best.q1; var p0 = best.p0; var p1 = best.p1;
    var idx = best.idx;
    if (idx[0] >= 8u) {
        let tq = q0; q0 = q1; q1 = tq;
        let tp = p0; p0 = p1; p1 = tp;
        for (var k = 0u; k < 16u; k++) { idx[k] = 15u - idx[k]; }
    }

    out_bits = array<u32, 4>(0u, 0u, 0u, 0u);
    var pos = 0u;
    put(pos, 7u, 64u); pos += 7u;
    put(pos, 7u, q0.x); pos += 7u; put(pos, 7u, q1.x); pos += 7u;
    put(pos, 7u, q0.y); pos += 7u; put(pos, 7u, q1.y); pos += 7u;
    put(pos, 7u, q0.z); pos += 7u; put(pos, 7u, q1.z); pos += 7u;
    put(pos, 7u, q0.w); pos += 7u; put(pos, 7u, q1.w); pos += 7u;
    put(pos, 1u, p0); pos += 1u; put(pos, 1u, p1); pos += 1u;
    put(pos, 3u, idx[0]); pos += 3u;
    for (var k = 1u; k < 16u; k++) { put(pos, 4u, idx[k]); pos += 4u; }
    blocks[bi] = vec4<u32>(out_bits[0], out_bits[1], out_bits[2], out_bits[3]);
}
