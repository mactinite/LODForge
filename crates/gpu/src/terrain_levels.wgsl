// Terrain LOD levels from one stitched image of a whole top (LOD32) tile.
//
// The cells of a top tile are first downscaled one by one to the finest level's pixels per cell
// and stitched into `base` (terrain_spec.wgsl downscale_v with the base parameters). Every level
// is then resized from the stitched image, so the filter sees across cell borders and leaves no
// cell grid. Diffuse is filtered in linear light, normals as vectors and renormalised.
//
// level_h:    Lanczos-3 horizontal pass, base (RGBA8) -> tmp (f16 x 4), rows of base.
// level_v:    Lanczos-3 vertical pass, tmp -> level image (tone curve, tile after tile).
// level_copy: a level at the base's own resolution (decode, tone curve, re-layout).

struct LevelParams {
    src_side: u32,         // base side (pixels)
    dst_side: u32,         // this level's side over the whole top tile (pixels)
    level: u32,            // cells per tile
    tile_size: u32,        // pixels per tile side
    tiles_per_side: u32,
    kind: u32,             // 0 diffuse, 1 normal
    gamma_inv: f32,
    brightness: f32,
    contrast: f32,
    _p1: u32,
    _p2: u32,
    _p3: u32,
};

@group(0) @binding(0) var<uniform> lp: LevelParams;
@group(0) @binding(1) var<storage, read> base: array<u32>;
@group(0) @binding(2) var<storage, read_write> tmp: array<vec2<u32>>;
// level_v reads tmp read-only, so wgpu puts a barrier between the passes.
@group(0) @binding(2) var<storage, read> tmp_in: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read_write> level_px: array<u32>;

const PI: f32 = 3.14159265358979;

fn lanczos3(x_in: f32) -> f32 {
    let x = abs(x_in);
    if (x < 1e-6) { return 1.0; }
    if (x >= 3.0) { return 0.0; }
    let px = PI * x;
    return 3.0 * sin(px) * sin(px / 3.0) / (px * px);
}

fn srgb_to_linear(v: vec3<f32>) -> vec3<f32> {
    return select(pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
}

fn linear_to_srgb(v_in: vec3<f32>) -> vec3<f32> {
    let v = clamp(v_in, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055, v * 12.92, v <= vec3<f32>(0.0031308));
}

// Base texel -> working space (linear colour, or a normal vector in -1..1).
fn decode(c: u32) -> vec3<f32> {
    let v = vec3<f32>(f32(c & 0xffu), f32((c >> 8u) & 0xffu), f32((c >> 16u) & 0xffu)) / 255.0;
    if (lp.kind == 1u) {
        return v * 2.0 - 1.0;
    }
    return srgb_to_linear(v);
}

// Working space -> stored 0..1 value (8-bit quantised), with the level's tone curve on diffuse.
fn encode(w: vec3<f32>) -> vec3<f32> {
    if (lp.kind == 1u) {
        let l = length(w);
        var n = vec3<f32>(0.0, 1.0, 0.0);   // up is green in terrain LOD normal maps
        if (l > 1e-6) { n = w / l; }
        return round((n * 0.5 + 0.5) * 255.0) / 255.0;
    }
    var v = round(linear_to_srgb(w) * 255.0) / 255.0;
    if (lp.brightness != 0.0 || lp.contrast != 0.0) {
        v = v * (1.0 + lp.contrast) + lp.brightness;
    }
    if (lp.gamma_inv != 1.0) {
        v = pow(max(v, vec3<f32>(0.0)), vec3<f32>(lp.gamma_inv));
    }
    return round(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0) / 255.0;
}

// Stitched (north-up) pixel (x, y) of this level -> index in the level image (tile after tile,
// tiles south first, rows north-up inside a tile).
fn level_index(x: u32, y: u32) -> u32 {
    let ts = lp.tile_size;
    let tx = x / ts;
    let ty = lp.tiles_per_side - 1u - y / ts;
    return (ty * lp.tiles_per_side + tx) * ts * ts + (y % ts) * ts + (x % ts);
}

fn store(x: u32, y: u32, w: vec3<f32>) {
    let o = vec3<u32>(encode(w) * 255.0 + 0.5);
    level_px[level_index(x, y)] = o.x | (o.y << 8u) | (o.z << 16u) | (255u << 24u);
}

fn pack4(v: vec3<f32>) -> vec2<u32> {
    return vec2<u32>(pack2x16float(v.xy), pack2x16float(vec2<f32>(v.z, 0.0)));
}

fn unpack4(p: vec2<u32>) -> vec3<f32> {
    return vec3<f32>(unpack2x16float(p.x), unpack2x16float(p.y).x);
}

@compute @workgroup_size(64, 1, 1)
fn level_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let row = gid.y;
    if (ox >= lp.dst_side || row >= lp.src_side) { return; }
    let scale = f32(lp.src_side) / f32(lp.dst_side);
    let k = max(scale, 1.0);
    let c = (f32(ox) + 0.5) * scale - 0.5;
    let first = i32(floor(c - 3.0 * k));
    let last = i32(ceil(c + 3.0 * k));
    var acc = vec3<f32>(0.0);
    var ws = 0.0;
    for (var j = first; j <= last; j++) {
        let w = lanczos3((f32(j) - c) / k);
        if (w == 0.0) { continue; }
        let sx = u32(clamp(j, 0, i32(lp.src_side) - 1));
        acc += decode(base[row * lp.src_side + sx]) * w;
        ws += w;
    }
    tmp[row * lp.dst_side + ox] = pack4(acc / ws);
}

@compute @workgroup_size(8, 8, 1)
fn level_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let oy = gid.y;
    if (ox >= lp.dst_side || oy >= lp.dst_side) { return; }
    let scale = f32(lp.src_side) / f32(lp.dst_side);
    let k = max(scale, 1.0);
    let c = (f32(oy) + 0.5) * scale - 0.5;
    let first = i32(floor(c - 3.0 * k));
    let last = i32(ceil(c + 3.0 * k));
    var acc = vec3<f32>(0.0);
    var ws = 0.0;
    for (var j = first; j <= last; j++) {
        let w = lanczos3((f32(j) - c) / k);
        if (w == 0.0) { continue; }
        let sy = u32(clamp(j, 0, i32(lp.src_side) - 1));
        acc += unpack4(tmp_in[sy * lp.dst_side + ox]) * w;
        ws += w;
    }
    store(ox, oy, acc / ws);
}

@compute @workgroup_size(8, 8, 1)
fn level_copy(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= lp.dst_side || y >= lp.dst_side) { return; }
    store(x, y, decode(base[y * lp.src_side + x]));
}
