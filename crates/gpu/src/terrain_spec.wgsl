// Terrain LOD diffuse and normal textures, following the reverse-engineered xLODGen pipeline
// (docs/re/xlodgen-terrain-textures.md). Mirrors lodforge_terrain::spec on the CPU; keep in sync.
// Every stored 8-bit value is truncated, as Vampyre's float pixel setters do.
//
// render_cell:  one thread per internal cell pixel (N = 24 * texPx, e.g. 384); output north-up.
//               kind 0 = diffuse (layers, vertex colours); kind 1 = normal (normal-map layers
//               baked onto the terrain normal).
// downscale_h:  Lanczos-3 horizontal pass N -> ppc for every rendered cell slot.
// downscale_v:  Lanczos-3 vertical pass per placement (cell position -> slot), 8-bit quantise,
//               tone curve, write into the level image (tiles stored one after another).

const NONE: u32 = 0xffffffffu;

struct RenderParams {
    cell_px: u32,
    tex_px: u32,
    vc_mult: f32,
    kind: u32,
};

struct Cell {
    vc_offset: u32,
    terrain_offset: u32,     // normal kind: 64*64 RGBA8 terrain normal (R=x, G=z, B=y), north-up
    _p1: u32,
    _p2: u32,
    base: vec4<u32>,         // texture index per quadrant (default already substituted)
    layer_start: vec4<u32>,
    layer_count: vec4<u32>,
};

struct Layer {
    tex: u32,
    splat_offset: u32,       // 33*33 floats (0..1, 8-bit values)
};

@group(0) @binding(0) var<uniform> rp: RenderParams;
@group(0) @binding(1) var<storage, read> textures: array<u32>;   // tex_px^2 RGBA8 per texture
@group(0) @binding(2) var<storage, read> cells: array<Cell>;
@group(0) @binding(3) var<storage, read> layers: array<Layer>;
@group(0) @binding(4) var<storage, read> splats: array<f32>;
@group(0) @binding(5) var<storage, read> vcolors: array<u32>;    // 33*33 RGBA8 per cell
@group(0) @binding(6) var<storage, read_write> cell_px: array<u32>;
@group(0) @binding(7) var<storage, read> terrain: array<u32>;    // 64*64 RGBA8 per cell

fn unpack(c: u32) -> vec4<f32> {
    return vec4<f32>(f32(c & 0xffu), f32((c >> 8u) & 0xffu), f32((c >> 16u) & 0xffu), f32(c >> 24u));
}

fn pack(v: vec4<f32>) -> u32 {
    let c = vec4<u32>(clamp(v, vec4<f32>(0.0), vec4<f32>(255.0)));
    return c.x | (c.y << 8u) | (c.z << 16u) | (c.w << 24u);
}

const PI: f32 = 3.14159265358979;

fn lanczos3(x_in: f32) -> f32 {
    let x = abs(x_in);
    if (x < 1e-6) { return 1.0; }
    if (x >= 3.0) { return 0.0; }
    let px = PI * x;
    return 3.0 * sin(px) * sin(px / 3.0) / (px * px);
}

// Upsampling Lanczos (33 -> dst) evaluated at destination pixel (dx, dy) of a 33x33 grid.
// `kind` 0: splat floats at `off`; 1: vertex colours (RGB 0..255) at `off`.
fn upsample33(kind: u32, off: u32, dst: u32, dx: u32, dy: u32) -> vec3<f32> {
    let scale = 33.0 / f32(dst);
    let cx = (f32(dx) + 0.5) * scale - 0.5;
    let cy = (f32(dy) + 0.5) * scale - 0.5;
    let x0 = i32(floor(cx - 3.0));
    let y0 = i32(floor(cy - 3.0));
    var acc = vec3<f32>(0.0);
    var wsx = 0.0;
    var wsy = 0.0;
    for (var i = 0; i < 8; i++) { wsx += lanczos3(f32(x0 + i) - cx); wsy += lanczos3(f32(y0 + i) - cy); }
    for (var j = 0; j < 8; j++) {
        let wy = lanczos3(f32(y0 + j) - cy);
        if (wy == 0.0) { continue; }
        let sy = u32(clamp(y0 + j, 0, 32));
        for (var i = 0; i < 8; i++) {
            let wx = lanczos3(f32(x0 + i) - cx);
            if (wx == 0.0) { continue; }
            let sx = u32(clamp(x0 + i, 0, 32));
            var v: vec3<f32>;
            if (kind == 0u) {
                v = vec3<f32>(splats[off + sy * 33u + sx]);
            } else {
                v = unpack(vcolors[off + sy * 33u + sx]).rgb / 255.0;
            }
            acc += v * (wx * wy);
        }
    }
    return acc / (wsx * wsy);
}

// Upsampling Lanczos of a cell's 64x64 terrain normal to `dst` px, at destination pixel (dx, dy)
// (north-up rows). Returns RGB in 0..1.
fn upsample_terrain(off: u32, dst: u32, dx: u32, dy: u32) -> vec3<f32> {
    let scale = 64.0 / f32(dst);
    let cx = (f32(dx) + 0.5) * scale - 0.5;
    let cy = (f32(dy) + 0.5) * scale - 0.5;
    let x0 = i32(floor(cx - 3.0));
    let y0 = i32(floor(cy - 3.0));
    var acc = vec3<f32>(0.0);
    var wsx = 0.0;
    var wsy = 0.0;
    for (var i = 0; i < 8; i++) { wsx += lanczos3(f32(x0 + i) - cx); wsy += lanczos3(f32(y0 + i) - cy); }
    for (var j = 0; j < 8; j++) {
        let wy = lanczos3(f32(y0 + j) - cy);
        if (wy == 0.0) { continue; }
        let sy = u32(clamp(y0 + j, 0, 63));
        for (var i = 0; i < 8; i++) {
            let wx = lanczos3(f32(x0 + i) - cx);
            if (wx == 0.0) { continue; }
            let sx = u32(clamp(x0 + i, 0, 63));
            acc += unpack(terrain[off + sy * 64u + sx]).rgb / 255.0 * (wx * wy);
        }
    }
    return acc / (wsx * wsy);
}

fn normalize_or(v: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    if (l < 1e-12) { return v; }
    return v / l;
}

// Bake of one pixel; t = normal-map texel, tn = terrain texel (R=x, G=z, B=y), both 0..1.
// Returns the encoded output (R=x, G=z, B=y) in 0..1.
fn bake_normal(t_in: vec3<f32>, tn: vec3<f32>) -> vec3<f32> {
    let t = normalize_or(t_in * 2.0 - 1.0);
    let n = normalize_or(vec3<f32>(tn.x, tn.z, tn.y) * 2.0 - 1.0);
    let a = normalize_or(cross(vec3<f32>(1.0, 0.0, 0.0), n));
    let tan = normalize_or(cross(n, a));
    let bit = -a;
    let o = normalize_or(t.x * tan + t.y * bit + t.z * n);
    return vec3<f32>(o.x + 1.0, o.z + 1.0, o.y + 1.0) / 2.0;
}

fn texel(tex: u32, x: u32, y: u32) -> vec4<f32> {
    let n = rp.tex_px;
    return unpack(textures[tex * n * n + (y % n) * n + (x % n)]);
}

@compute @workgroup_size(8, 8, 1)
fn render_cell(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = rp.cell_px;
    let x = gid.x;
    let y = gid.y;                 // row 0 = south while building
    let slot = gid.z;
    if (x >= n || y >= n) { return; }
    let q2 = n / 2u;
    let qx = select(0u, 1u, x >= q2);
    let qy = select(0u, 1u, y >= q2);
    let q = qy * 2u + qx;
    let lx = x - qx * q2;
    let ly = y - qy * q2;
    let cell = cells[slot];

    let base = texel(cell.base[q], lx, ly).rgb;      // 0..255
    var acc = vec3<f32>(0.0);
    var acc_a = 0.0;
    let big = q2 + q2 / 32u;
    let cx0 = (big - q2) / 2u;
    let start = cell.layer_start[q];
    for (var i = 0u; i < cell.layer_count[q]; i++) {
        let l = layers[start + i];
        let m = upsample33(0u, l.splat_offset, big, lx + cx0, ly + cx0 + 1u).x;
        let m8 = trunc(clamp(m, 0.0, 1.0) * 255.0);
        let t = texel(l.tex, lx, ly).rgb;
        acc = min(acc + trunc(t * m8 / 255.0), vec3<f32>(255.0));
        acc_a = min(acc_a + m8, 255.0);
    }
    var out = trunc(clamp(acc + base * (1.0 - acc_a / 255.0), vec3<f32>(0.0), vec3<f32>(255.0)));
    let ny = n - 1u - y;   // north-up row

    if (rp.kind == 1u) {
        // Normal: the composite is the landscape normal map (north-up after the flip).
        let tn = trunc(clamp(upsample_terrain(cell.terrain_offset, n, x, ny), vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0) / 255.0;
        out = trunc(clamp(bake_normal(out / 255.0, tn), vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    } else if (cell.vc_offset != NONE) {
        let vbig = n * 33u / 32u;
        let vo = (vbig - n) / 2u;
        let vc = trunc(clamp(upsample33(1u, cell.vc_offset, vbig, x + vo, y + vo), vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0) / 255.0;
        let m = out * vc;
        out = trunc(clamp(m + (out - m) * (1.0 - rp.vc_mult), vec3<f32>(0.0), vec3<f32>(255.0)));
    }
    // Flip to north-up.
    cell_px[slot * n * n + ny * n + x] = pack(vec4<f32>(out, 255.0));
}

// ---- downscale ---------------------------------------------------------------------------

struct DownParams {
    cell_px: u32,      // source size N
    ppc: u32,          // destination pixels per cell
    level: u32,        // cells per tile side
    tile_size: u32,    // ppc * level
    tiles_per_side: u32,
    round_down: u32,   // 1: the cell downscale rounds (diffuse), 0: truncates (normal)
    gamma_inv: f32,    // 1 / gamma
    brightness: f32,   // B/100
    contrast: f32,     // C/100
    _p1: u32,
    _p2: u32,
    _p3: u32,
};

// x, y: cell position inside the top tile (cells); slot: rendered cell index.
struct Placement {
    x: u32,
    y: u32,
    slot: u32,
    _p: u32,
};

@group(0) @binding(0) var<uniform> dp: DownParams;
@group(0) @binding(1) var<storage, read> src_px: array<u32>;          // rendered cells
@group(0) @binding(2) var<storage, read_write> tmp: array<vec4<f32>>; // slot * N * ppc
// downscale_v reads the same buffer read-only: the storage write -> read transition makes wgpu
// put a barrier between the passes (two read_write uses would not get one).
@group(0) @binding(2) var<storage, read> tmp_in: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> placements: array<Placement>;
@group(0) @binding(4) var<storage, read_write> level_px: array<u32>;  // tiles one after another

// Lanczos-3 weights for destination sample `d` of a `src` -> `dst` downscale.
fn down_center(d: u32, src: u32, dst: u32) -> f32 {
    return (f32(d) + 0.5) * (f32(src) / f32(dst)) - 0.5;
}

@compute @workgroup_size(64, 1, 1)
fn downscale_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let row = gid.y;
    let slot = gid.z;
    let n = dp.cell_px;
    if (ox >= dp.ppc || row >= n) { return; }
    let scale = f32(n) / f32(dp.ppc);
    let k = max(scale, 1.0);
    let support = 3.0 * k;
    let c = down_center(ox, n, dp.ppc);
    let first = i32(floor(c - support));
    let last = i32(ceil(c + support));
    var acc = vec4<f32>(0.0);
    var ws = 0.0;
    for (var j = first; j <= last; j++) {
        let w = lanczos3((f32(j) - c) / k);
        if (w == 0.0) { continue; }
        let sx = u32(clamp(j, 0, i32(n) - 1));
        acc += unpack(src_px[slot * n * n + row * n + sx]) * w;
        ws += w;
    }
    tmp[(slot * n + row) * dp.ppc + ox] = acc / ws;
}

@compute @workgroup_size(8, 8, 1)
fn downscale_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let oy = gid.y;
    let pl = placements[gid.z];
    let n = dp.cell_px;
    if (ox >= dp.ppc || oy >= dp.ppc) { return; }
    let scale = f32(n) / f32(dp.ppc);
    let k = max(scale, 1.0);
    let support = 3.0 * k;
    let c = down_center(oy, n, dp.ppc);
    let first = i32(floor(c - support));
    let last = i32(ceil(c + support));
    var acc = vec4<f32>(0.0);
    var ws = 0.0;
    for (var j = first; j <= last; j++) {
        let w = lanczos3((f32(j) - c) / k);
        if (w == 0.0) { continue; }
        let sy = u32(clamp(j, 0, i32(n) - 1));
        acc += tmp_in[(pl.slot * n + sy) * dp.ppc + ox] * w;
        ws += w;
    }
    let d = clamp(acc.rgb / ws, vec3<f32>(0.0), vec3<f32>(255.0));
    var v = select(trunc(d), round(d), dp.round_down != 0u) / 255.0;
    // Tone curve on the (8-bit) tile: brightness/contrast, then gamma.
    if (dp.brightness != 0.0 || dp.contrast != 0.0) {
        v = v * (1.0 + dp.contrast) + dp.brightness;
    }
    if (dp.gamma_inv != 1.0) {
        v = pow(max(v, vec3<f32>(0.0)), vec3<f32>(dp.gamma_inv));
    }
    let o = trunc(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    // Placement -> tile and position (row 0 = north inside the tile).
    let tx = pl.x / dp.level;
    let ty = pl.y / dp.level;
    let lx = pl.x % dp.level;
    let ly = pl.y % dp.level;
    let tile = ty * dp.tiles_per_side + tx;
    let px = lx * dp.ppc + ox;
    let py = (dp.level - 1u - ly) * dp.ppc + oy;
    level_px[tile * dp.tile_size * dp.tile_size + py * dp.tile_size + px] = pack(vec4<f32>(o, 255.0));
}
