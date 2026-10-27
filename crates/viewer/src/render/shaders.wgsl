// SPDX-License-Identifier: GPL-3.0-only
// LODForge viewer shaders: LOD meshes, classic tree billboards and overlay lines.

struct Globals {
    view_proj: mat4x4<f32>,
    // xyz camera position; w = 1 for the top-down orthographic overview.
    cam_pos: vec4<f32>,
    // xyz toward the sun; w = ambient share.
    sun_dir: vec4<f32>,
    // x = shading mode, y = normal maps on, z = alpha test override (<0: as the material),
    // w = snow / ash LOD materials on.
    params: vec4<f32>,
    level_colors: array<vec4<f32>, 4>,
    kind_colors: array<vec4<f32>, 6>,
    // Per snow class 1..4, three vec4: (falloff scale, falloff bias, 1 / noise UV scale, HD),
    // (colour, -), (projection vector, -).
    snow: array<vec4<f32>, 12>,
    inv_view_proj: mat4x4<f32>,
    // Weather and water (sRGB colours, as the records store them): 0 fog (near, far, power,
    // max), 1 fog near colour, 2 fog far colour, 3 sky upper, 4 sky lower, 5 horizon,
    // 6 sunlight, 7 ambient, 8 water deep, 9 water reflected sky, 10 water (fresnel,
    // reflectivity, -, -).
    atmo: array<vec4<f32>, 11>,
    // x = weather looks on.
    atmo_on: vec4<f32>,
    // LODForge Runtime tree rules: 0 (mesh tier end, 0 = off; hand-over band; frame dither;
    // alpha ref), 1 (fade-out start, end; thin start, end), 2 (thin keep; blend frames; rules on;
    // apply type tint).
    rt: array<vec4<f32>, 3>,
};

// With weather looks on, lighting, fog and the sky are computed the way the game does without
// linear lighting: in gamma (sRGB) space, then converted back to the linear colour this shader
// writes.
fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn atmo_on() -> bool {
    return g.atmo_on.x > 0.5;
}

// The weather's light on a surface (gamma space): ambient plus sunlight by N.L.
fn weather_light(n: vec3<f32>) -> vec3<f32> {
    let d = max(dot(n, normalize(g.sun_dir.xyz)), 0.0);
    return g.atmo[7].rgb + g.atmo[6].rgb * d;
}

// A linear albedo lit by the weather; returns linear colour.
fn weather_shade(base: vec3<f32>, light: vec3<f32>) -> vec3<f32> {
    return to_linear(to_gamma(base) * light);
}

// The game's distance fog: amount = min(max, ((d - near) / (far - near)) ^ k), colour from
// the near to the far fog colour by the same factor, blended in gamma space. k = 1 / the
// weather's fog power: with the power itself (0.4 for SkyrimClear_A) Whiterun at 25000 units
// would be 63% fogged, where in-game captures show it lightly hazed and the hills past 50000
// heavily, which 1 / power reproduces.
fn apply_fog(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let f = g.atmo[0];
    let d = distance(g.cam_pos.xyz, world);
    let t = pow(clamp((d - f.x) / max(f.y - f.x, 1.0), 0.0, 1.0), 1.0 / max(f.z, 0.05));
    let amount = min(t, f.w);
    let fog = mix(g.atmo[1].rgb, g.atmo[2].rgb, t);
    return to_linear(mix(to_gamma(c), fog, amount));
}

// Sky colour for a view direction (linear), blended in gamma space: the far fog colour at the
// horizon (the game fogs the sky dome there), then horizon, sky lower and sky upper toward the
// zenith. Clouds are not drawn.
fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let e = dir.z;
    let base = mix(g.atmo[2].rgb, g.atmo[5].rgb, smoothstep(0.0, 0.08, e));
    let low = mix(base, g.atmo[4].rgb, smoothstep(0.08, 0.25, e));
    return to_linear(mix(low, g.atmo[3].rgb, smoothstep(0.25, 0.8, e)));
}

struct Material {
    flags: u32,
    threshold: f32,
    emissive_mult: f32,
    // Low 8 bits: shape kind; bits 8..: level index (0 = LOD4).
    kind_level: u32,
    emissive: vec4<f32>,
    // UV scale xy, offset zw.
    uv: vec4<f32>,
    // Highlight colour, a = strength.
    tint: vec4<f32>,
};

const F_ALPHA_TEST: u32 = 1u;
const F_DOUBLE_SIDED: u32 = 2u;
const F_VERTEX_COLORS: u32 = 4u;
const F_NORMAL_MAP: u32 = 8u;
const F_EMIT: u32 = 16u;
const F_UNLIT: u32 = 32u;
const F_TERRAIN: u32 = 64u;
const F_WATER: u32 = 128u;
const F_LARGE_REF: u32 = 256u;
const F_HAS_NORMALS: u32 = 1024u;
const F_HAS_TANGENTS: u32 = 2048u;
const F_CLAMP: u32 = 4096u;

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var t_proj_diffuse: texture_2d<f32>;
@group(0) @binding(2) var t_proj_noise: texture_2d<f32>;
@group(0) @binding(3) var samp_proj: sampler;
@group(1) @binding(0) var<storage, read> materials: array<Material>;
@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var t_normal: texture_2d<f32>;
@group(2) @binding(2) var samp: sampler;
@group(2) @binding(3) var samp_clamp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec4<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) bitangent: vec4<f32>,
    @location(5) color: vec4<f32>,
    @builtin(instance_index) inst: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) tangent: vec3<f32>,
    @location(4) bitangent: vec3<f32>,
    @location(5) color: vec4<f32>,
    @location(6) @interpolate(flat) inst: u32,
};

@vertex
fn vs_mesh(in: VsIn) -> VsOut {
    var out: VsOut;
    let m = materials[in.inst];
    out.clip = g.view_proj * vec4<f32>(in.pos, 1.0);
    out.world = in.pos;
    out.uv = in.uv * m.uv.xy + m.uv.zw;
    out.normal = in.normal.xyz;
    out.tangent = in.tangent.xyz;
    out.bitangent = in.bitangent.xyz;
    out.color = in.color;
    out.inst = in.inst;
    return out;
}

// Runtime mesh packs (near-tier tree meshes, repeated statics): model-space vertices moved by
// the instance's rows; the vertex's page picks its material (fs_mesh).
struct PackIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec4<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) bitangent: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) page: u32,
    @location(7) r0: vec4<f32>,
    @location(8) r1: vec4<f32>,
    @location(9) r2: vec4<f32>,
};

fn pack_dir(in: PackIn, d: vec3<f32>) -> vec3<f32> {
    return normalize(vec3<f32>(dot(in.r0.xyz, d), dot(in.r1.xyz, d), dot(in.r2.xyz, d)) + vec3<f32>(0.0, 0.0, 1e-6));
}

@vertex
fn vs_pack(in: PackIn) -> VsOut {
    var out: VsOut;
    let p = vec4<f32>(in.pos, 1.0);
    let w = vec3<f32>(dot(in.r0, p), dot(in.r1, p), dot(in.r2, p));
    out.clip = g.view_proj * vec4<f32>(w, 1.0);
    out.world = w;
    out.uv = in.uv;
    out.normal = pack_dir(in, in.normal.xyz);
    out.tangent = pack_dir(in, in.tangent.xyz);
    out.bitangent = pack_dir(in, in.bitangent.xyz);
    out.color = in.color;
    out.inst = in.page;
    return out;
}

fn lit(n: vec3<f32>) -> f32 {
    let ambient = g.sun_dir.w;
    let d = max(dot(n, normalize(g.sun_dir.xyz)), 0.0);
    return ambient + (1.0 - ambient) * d;
}

fn shade(base: vec3<f32>, n: vec3<f32>, m: Material, emissive: vec3<f32>, raw_normal: vec3<f32>) -> vec3<f32> {
    let mode = i32(g.params.x);
    let kind = m.kind_level & 255u;
    let level = (m.kind_level >> 8u) & 3u;
    var c: vec3<f32>;
    if (mode == 1) {
        c = base;
    } else if (mode == 2) {
        c = n * 0.5 + 0.5;
    } else if (mode == 3) {
        c = vec3<f32>(0.75) * lit(n);
    } else if (mode == 4) {
        c = g.level_colors[level].rgb * lit(n);
    } else if (mode == 5) {
        c = g.kind_colors[min(kind, 5u)].rgb * lit(n);
    } else if (mode == 6) {
        c = raw_normal;
    } else {
        if ((m.flags & F_UNLIT) != 0u) {
            c = base;
        } else if (atmo_on()) {
            c = weather_shade(base, weather_light(n)) + emissive;
        } else {
            c = base * lit(n) + emissive;
        }
    }
    return mix(c, m.tint.rgb, m.tint.a);
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let m = materials[in.inst];
    // Every sample and derivative first, in uniform control flow.
    // Both address modes are sampled (uniform control flow) and the shape's clamp mode picks one.
    let clamp_uv = (m.flags & F_CLAMP) != 0u;
    var base = select(textureSample(t_diffuse, samp, in.uv), textureSample(t_diffuse, samp_clamp, in.uv), clamp_uv);
    let t = select(textureSample(t_normal, samp, in.uv), textureSample(t_normal, samp_clamp, in.uv), clamp_uv).xyz * 2.0 - 1.0;
    let ddx = dpdx(in.world);
    let ddy = dpdy(in.world);
    // Projected snow / ash inputs (triplanar over world space, as the game's shader), sampled
    // here in uniform control flow; used below only for snow-class shapes.
    let snow_class = (m.kind_level >> 16u) & 7u;
    let sc = (max(snow_class, 1u) - 1u) * 3u;
    let sp = g.snow[sc];
    let tw0 = pow(abs(in.normal) + vec3<f32>(1e-4), vec3<f32>(6.0));
    let tw = tw0 / (tw0.x + tw0.y + tw0.z);
    let pn = in.world * sp.z;
    let proj_noise = textureSample(t_proj_noise, samp_proj, pn.yz).x * tw.x
        + textureSample(t_proj_noise, samp_proj, pn.xz).x * tw.y
        + textureSample(t_proj_noise, samp_proj, pn.xy).x * tw.z;
    // fProjectedUVDiffuseNormalTilingScale (0.2) times the noise scale.
    let pd = in.world * (sp.z * 0.2);
    let proj_diffuse = textureSample(t_proj_diffuse, samp_proj, pd.yz).rgb * tw.x
        + textureSample(t_proj_diffuse, samp_proj, pd.xz).rgb * tw.y
        + textureSample(t_proj_diffuse, samp_proj, pd.xy).rgb * tw.z;
    // The alpha test reads the texture's alpha: in LOD the vertex alpha carries the snow / ash
    // amount (objSnow*), not opacity.
    let tex_alpha = base.a;
    if ((m.flags & F_VERTEX_COLORS) != 0u) {
        base = base * in.color;
    }
    var threshold = m.threshold;
    var test = (m.flags & F_ALPHA_TEST) != 0u;
    if (g.params.z >= 0.0) {
        test = true;
        threshold = g.params.z;
    }
    if (test && tex_alpha < threshold) {
        discard;
    }
    let to_cam = normalize(g.cam_pos.xyz - in.world);
    var n: vec3<f32>;
    let normal_maps = g.params.y > 0.5 && (m.flags & F_NORMAL_MAP) != 0u;
    if ((m.flags & F_TERRAIN) != 0u && normal_maps) {
        // Terrain LOD normal maps hold model-space normals (the mesh carries none) with the up
        // axis in the green channel: stored (x, z, y) for a Z-up world.
        n = normalize(vec3<f32>(t.x, t.z, t.y));
    } else if ((m.flags & F_HAS_NORMALS) != 0u) {
        n = normalize(in.normal);
        if (normal_maps && (m.flags & F_HAS_TANGENTS) != 0u) {
            n = normalize(normalize(in.tangent) * t.x + normalize(in.bitangent) * t.y + n * t.z);
        }
    } else {
        // Flat normal from the surface itself.
        n = normalize(cross(ddx, ddy));
        if (dot(n, to_cam) < 0.0) {
            n = -n;
        }
    }
    if (!front && (m.flags & F_DOUBLE_SIDED) != 0u) {
        n = -n;
    }
    if (snow_class != 0u && g.params.w > 0.5) {
        // The game's projected material: weight = -falloff scale * noise
        // + (dot(normal, projection) * vertex alpha - falloff bias); HD shapes take no alpha
        // factor but add (alpha - 0.5) * 2.5. Projected diffuse normals on: a smooth blend.
        let vc_alpha = select(1.0, in.color.a, (m.flags & F_VERTEX_COLORS) != 0u);
        let hd = sp.w > 0.5;
        let col = g.snow[sc + 1u].rgb;
        let proj_dir = normalize(g.snow[sc + 2u].xyz);
        var w = -sp.x * proj_noise + (dot(n, proj_dir) * select(vc_alpha, 1.0, hd) - sp.y);
        if (hd) {
            w = w + (vc_alpha - 0.5) * 2.5;
        }
        let weight = smoothstep(0.0, 1.0, 5.0 * (0.1 + w));
        // Brightness: the projected texture times twice the material colour's luminance, in
        // gamma space (the vanilla x2 colour convention). Calibrated on an in-game capture of the
        // Solitude glacier (snow tops about twice the ice in sRGB, with the texture's hue); the
        // game's snow specular is not modelled, so the colour's hue is not applied.
        let gain = 2.0 * dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
        let snow_rgb = proj_diffuse * pow(gain, 2.2);
        base = vec4<f32>(mix(base.rgb, snow_rgb, weight), base.a);
    }
    var emissive = vec3<f32>(0.0);
    if ((m.flags & F_EMIT) != 0u) {
        emissive = m.emissive.rgb * m.emissive_mult * base.rgb;
    }
    var rgb = shade(base.rgb, n, m, emissive, t * 0.5 + 0.5);
    var a = 1.0;
    let textured = i32(g.params.x) == 0;
    if ((m.flags & F_WATER) != 0u) {
        if (atmo_on() && textured) {
            // Distant water: the deep colour, giving way to the reflected sky (sky lower times
            // the weather's water multiplier) by a Schlick fresnel term toward grazing angles.
            let cos_v = clamp(dot(to_cam, vec3<f32>(0.0, 0.0, 1.0)), 0.0, 1.0);
            let f0 = g.atmo[10].x;
            let fres = (f0 + (1.0 - f0) * pow(1.0 - cos_v, 5.0)) * g.atmo[10].y;
            let deep = g.atmo[8].rgb * weather_light(vec3<f32>(0.0, 0.0, 1.0));
            rgb = to_linear(mix(deep, g.atmo[9].rgb, clamp(fres, 0.0, 1.0)));
        } else {
            rgb = mix(rgb, vec3<f32>(0.1, 0.3, 0.5), 0.6);
            a = 0.6;
        }
    }
    if (atmo_on() && textured) {
        rgb = apply_fog(rgb, in.world);
    }
    return vec4<f32>(rgb, a);
}

// Solid colour over the mesh layout (selection wireframe).
@fragment
fn fs_solid(in: VsOut) -> @location(0) vec4<f32> {
    let m = materials[in.inst];
    return vec4<f32>(m.tint.rgb, 1.0);
}

// Classic tree LOD billboards: one quad per instance, built here, facing the camera in the
// free view and lying flat (as a footprint) in the top-down overview.
struct TreeIn {
    @location(0) pos: vec3<f32>,
    @location(1) rotation: f32,
    @location(2) size: vec2<f32>,
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
    @location(5) flags: u32,
    @builtin(vertex_index) vi: u32,
};

struct TreeOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) flags: u32,
    @location(2) world: vec3<f32>,
};

@group(1) @binding(0) var tree_diffuse: texture_2d<f32>;
@group(1) @binding(1) var tree_normal: texture_2d<f32>;
@group(1) @binding(2) var tree_samp: sampler;
@group(1) @binding(3) var samp_tree_clamp: sampler;

@vertex
fn vs_tree(in: TreeIn) -> TreeOut {
    var out: TreeOut;
    let cx = select(-0.5, 0.5, (in.vi & 1u) == 1u);
    let cy = select(0.0, 1.0, in.vi >= 2u);
    var world: vec3<f32>;
    if (g.cam_pos.w > 0.5) {
        // Overview: a flat square footprint of the billboard's width.
        world = vec3<f32>(in.pos.x + cx * in.size.x, in.pos.y + (cy - 0.5) * in.size.x, in.pos.z + 1.0);
    } else {
        var to_cam = g.cam_pos.xy - in.pos.xy;
        let len = length(to_cam);
        var dir = vec2<f32>(0.0, 1.0);
        if (len > 1e-3) {
            dir = to_cam / len;
        }
        let right = vec2<f32>(dir.y, -dir.x);
        world = vec3<f32>(in.pos.xy + right * cx * in.size.x, in.pos.z + cy * in.size.y);
    }
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.uv = mix(in.uv_min, in.uv_max, vec2<f32>(cx + 0.5, 1.0 - cy));
    out.flags = in.flags;
    out.world = world;
    return out;
}

@fragment
fn fs_tree(in: TreeOut) -> @location(0) vec4<f32> {
    var base = textureSample(tree_diffuse, tree_samp, in.uv);
    let _n = textureSample(tree_normal, tree_samp, in.uv);
    if ((in.flags & 2u) != 0u) {
        base = vec4<f32>(1.0, 0.0, 1.0, 1.0);
    }
    var threshold = 0.5;
    if (g.params.z >= 0.0) {
        threshold = g.params.z;
    }
    if (base.a < threshold) {
        discard;
    }
    let mode = i32(g.params.x);
    var c = base.rgb;
    if (mode == 0 && atmo_on()) {
        // No normal: half the sunlight, as a vertical surface turned partly toward the sun.
        c = apply_fog(weather_shade(c, g.atmo[7].rgb + g.atmo[6].rgb * 0.5), in.world);
    } else if (mode == 0 || mode == 3) {
        // No normal: a billboard faces the viewer, light it as a vertical surface toward the sun.
        let ambient = g.sun_dir.w;
        c = c * (ambient + (1.0 - ambient) * 0.6);
    } else if (mode == 2) {
        c = vec3<f32>(0.5, 0.5, 1.0);
    } else if (mode == 4) {
        c = g.level_colors[0].rgb;
    } else if (mode == 5) {
        c = g.kind_colors[3].rgb;
    }
    if ((in.flags & 1u) != 0u) {
        c = mix(c, vec3<f32>(1.0, 0.6, 0.1), 0.6);
    }
    return vec4<f32>(c, 1.0);
}

// LODForge runtime impostors (docs/runtime/impostors.md, the shader contract; the math mirrors
// lodforge-impostor `hemioct.rs`): one camera-facing quad per instance, the three hemi-
// octahedral frames enclosing the view direction blended by barycentric weight, lit with the
// baked model-space normals.
struct ImpIn {
    // xyz position, w yaw (radians; world = Rz(-yaw) * model).
    @location(0) pos_yaw: vec4<f32>,
    // xyz model origin -> impostor centre, w reference scale.
    @location(1) center_scale: vec4<f32>,
    // The type's rectangle on its page (u0, v0, du, dv).
    @location(2) rect: vec4<f32>,
    // Quad size (model units), frames per side, frame size (px), flags.
    @location(3) misc: vec4<f32>,
    // rgb: brightness * tint; w: 1 when the albedo is pre-lit.
    @location(4) tint: vec4<f32>,
    // x: reference hash, y: flags (bit 0: the tree has a mesh-tier model).
    @location(5) ids: vec4<u32>,
    @builtin(vertex_index) vi: u32,
};

struct ImpOut {
    @builtin(position) clip: vec4<f32>,
    // Per frame: xy grid index, zw frame-local uv (y down).
    @location(0) f0: vec4<f32>,
    @location(1) f1: vec4<f32>,
    @location(2) f2: vec4<f32>,
    @location(3) w: vec3<f32>,
    @location(4) world: vec3<f32>,
    @location(5) @interpolate(flat) rect: vec4<f32>,
    // x grid, y frame px, z yaw.
    @location(6) @interpolate(flat) info: vec3<f32>,
    @location(7) @interpolate(flat) tint: vec4<f32>,
    // Distance fade (1 = fully shown), dithered per pixel as the game's depth pass does.
    @location(8) @interpolate(flat) fade: f32,
};

// The runtime's hash ranks (InstanceCull.hlsl): ((hash * k) >> 8) / 2^24.
fn rt_rank(hash: u32, k: u32) -> f32 {
    return f32((hash * k) >> 8u) / 16777216.0;
}

// Fade of a runtime tree impostor at `dist` (0: not drawn): fade-out band, hash-ranked thinning
// and the hand-over to the mesh tier (a tree with a mesh is drawn by vs_pack on the near side
// of its own point in the band).
fn rt_tree_fade(dist: f32, hash: u32, has_mesh: bool) -> f32 {
    if (g.rt[2].z < 0.5) {
        return 1.0;
    }
    var fade = 1.0;
    let f = g.rt[1];
    if (f.y > f.x) {
        fade = clamp((f.y - dist) / (f.y - f.x), 0.0, 1.0);
    }
    let t = clamp((dist - f.z) / max(f.w - f.z, 1.0), 0.0, 1.0);
    if (rt_rank(hash, 1u) >= mix(1.0, g.rt[2].x, t)) {
        return 0.0;
    }
    let end = g.rt[0].x;
    if (has_mesh && end > 0.0) {
        let mesh_t = clamp((dist - (end - g.rt[0].y)) / max(g.rt[0].y, 1.0), 0.0, 1.0);
        if (rt_rank(hash, 2654435761u) >= mesh_t) {
            return 0.0;
        }
    }
    return fade;
}

// 4x4 Bayer threshold of a pixel, in (0, 1).
fn bayer4(p: vec2<f32>) -> f32 {
    let x = u32(p.x) & 3u;
    let y = u32(p.y) & 3u;
    var m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (m[y * 4u + x] + 0.5) / 16.0;
}

fn hemi_oct_encode(d: vec3<f32>) -> vec2<f32> {
    let n = abs(d.x) + abs(d.y) + d.z;
    let x = d.x / n;
    let y = d.y / n;
    return vec2<f32>(x + y, x - y) * 0.5 + 0.5;
}

fn hemi_oct_decode(uv: vec2<f32>) -> vec3<f32> {
    let s = uv.x * 2.0 - 1.0;
    let t = uv.y * 2.0 - 1.0;
    let x = (s + t) * 0.5;
    let y = (s - t) * 0.5;
    return normalize(vec3<f32>(x, y, 1.0 - abs(x) - abs(y)));
}

// Frame basis for a direction toward the camera: right x up = dir; top-down: +X, +Y.
fn imp_right(dir: vec3<f32>) -> vec3<f32> {
    if (abs(dir.z) < 0.999) {
        return normalize(cross(vec3<f32>(0.0, 0.0, 1.0), dir));
    }
    return vec3<f32>(1.0, 0.0, 0.0);
}

fn imp_up(dir: vec3<f32>, right: vec3<f32>) -> vec3<f32> {
    if (abs(dir.z) < 0.999) {
        return cross(dir, right);
    }
    return vec3<f32>(0.0, 1.0, 0.0);
}

fn frame_dir(idx: vec2<f32>, grid: f32) -> vec3<f32> {
    return hemi_oct_decode(idx / (grid - 1.0));
}

// Frame-local uv (y down) of a model-space offset from the impostor centre.
fn frame_uv(p: vec3<f32>, idx: vec2<f32>, grid: f32, quad: f32) -> vec2<f32> {
    let d = frame_dir(idx, grid);
    let r = imp_right(d);
    let u = imp_up(d, r);
    return vec2<f32>(0.5 + dot(p, r) / quad, 0.5 - dot(p, u) / quad);
}

// Rz(a) applied to v.
fn rot_z(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * v.x - s * v.y, s * v.x + c * v.y, v.z);
}

@vertex
fn vs_impostor(in: ImpIn) -> ImpOut {
    var out: ImpOut;
    let pos = in.pos_yaw.xyz;
    let yaw = in.pos_yaw.w;
    let center = in.center_scale.xyz;
    let scale = max(in.center_scale.w, 1e-3);
    let quad = in.misc.x;
    let grid = in.misc.y;
    // Camera in model space (model = Rz(yaw) * world), relative to the impostor centre.
    var dir = vec3<f32>(0.0, 0.0, 1.0);
    if (g.cam_pos.w < 0.5) {
        let cam = rot_z(g.cam_pos.xyz - pos, yaw) / scale - center;
        let l = length(cam);
        if (l > 1e-3) {
            dir = cam / l;
        }
    }
    dir = normalize(vec3<f32>(dir.xy, max(dir.z, 0.02)));
    let max_idx = grid - 1.0;
    let gp = clamp(hemi_oct_encode(dir), vec2<f32>(0.0), vec2<f32>(1.0)) * max_idx;
    let cx = select(-0.5, 0.5, (in.vi & 1u) == 1u);
    let cy = select(-0.5, 0.5, in.vi >= 2u);
    let hash = in.ids.x;
    var p: vec3<f32>;
    if (g.rt[2].y > 0.5) {
        // Shader contract: the enclosing triangle of grid-vertex frames, blended by barycentric
        // weight, on a quad facing the camera in the view direction's own basis.
        let b = min(floor(gp), vec2<f32>(max_idx - 1.0));
        let f = gp - b;
        var i0: vec2<f32>;
        let i1 = b + vec2<f32>(1.0, 0.0);
        let i2 = b + vec2<f32>(0.0, 1.0);
        var w: vec3<f32>;
        if (f.x + f.y <= 1.0) {
            i0 = b;
            w = vec3<f32>(1.0 - f.x - f.y, f.x, f.y);
        } else {
            i0 = b + vec2<f32>(1.0, 1.0);
            w = vec3<f32>(f.x + f.y - 1.0, 1.0 - f.y, 1.0 - f.x);
        }
        let right = imp_right(dir);
        let up = imp_up(dir, right);
        p = (right * cx + up * cy) * quad;
        out.f0 = vec4<f32>(i0, frame_uv(p, i0, grid, quad));
        out.f1 = vec4<f32>(i1, frame_uv(p, i1, grid, quad));
        out.f2 = vec4<f32>(i2, frame_uv(p, i2, grid, quad));
        out.w = w;
    } else {
        // The runtime (ImpostorVS.hlsl): the nearest frame, switching at a per-tree jittered
        // angle, on a quad in that frame's own capture plane.
        let jitter = (vec2<f32>(f32(hash & 0xFFFFu), f32(hash >> 16u)) / 65535.0 - 0.5) * g.rt[0].z;
        let idx = clamp(floor(gp + 0.5 + jitter), vec2<f32>(0.0), vec2<f32>(max_idx));
        let fd = frame_dir(idx, grid);
        let right = imp_right(fd);
        let up = imp_up(fd, right);
        p = (right * cx + up * cy) * quad;
        let inset = 1.0 - 1.0 / max(in.misc.z, 2.0);
        let local = vec2<f32>(0.5 + cx * inset, 0.5 - cy * inset);
        out.f0 = vec4<f32>(idx, local);
        out.f1 = out.f0;
        out.f2 = out.f0;
        out.w = vec3<f32>(1.0, 0.0, 0.0);
    }
    var world = pos + rot_z((center + p) * scale, -yaw);
    // Distance rules, measured from the camera to the impostor centre's height above the base.
    let dist = distance(g.cam_pos.xyz, pos + vec3<f32>(0.0, 0.0, center.z * scale));
    var fade = 1.0;
    if (g.cam_pos.w < 0.5) {
        fade = rt_tree_fade(dist, hash, (in.ids.y & 1u) != 0u);
    }
    out.fade = fade;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    if (fade <= 0.0) {
        // Not drawn: every corner outside the clip volume.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.world = world;
    out.rect = in.rect;
    out.info = vec3<f32>(grid, in.misc.z, yaw);
    out.tint = in.tint;
    return out;
}

// Atlas uv of a frame-local uv, clamped half a texel inside the frame's cell.
fn imp_atlas_uv(fr: vec4<f32>, rect: vec4<f32>, grid: f32, frame_px: f32) -> vec2<f32> {
    let inset = 0.5 / frame_px;
    let l = clamp(fr.zw, vec2<f32>(inset), vec2<f32>(1.0 - inset));
    return rect.xy + (fr.xy + l) * rect.zw / grid;
}

// Canopy-shell direction of one frame (model space, unnormalised): from the centre to the
// texel's point on the frame plane, pushed toward the camera by the baked depth.
fn imp_shell(fr: vec4<f32>, depth: f32, grid: f32) -> vec3<f32> {
    let d = frame_dir(fr.xy, grid);
    let r = imp_right(d);
    let u = imp_up(d, r);
    return r * (fr.z - 0.5) + u * (0.5 - fr.w) + d * (0.5 - depth);
}

@fragment
fn fs_impostor(in: ImpOut) -> @location(0) vec4<f32> {
    let grid = in.info.x;
    let fpx = in.info.y;
    let uv0 = imp_atlas_uv(in.f0, in.rect, grid, fpx);
    let uv1 = imp_atlas_uv(in.f1, in.rect, grid, fpx);
    let uv2 = imp_atlas_uv(in.f2, in.rect, grid, fpx);
    let a0 = textureSample(tree_diffuse, samp_tree_clamp, uv0);
    let a1 = textureSample(tree_diffuse, samp_tree_clamp, uv1);
    let a2 = textureSample(tree_diffuse, samp_tree_clamp, uv2);
    let n0 = textureSample(tree_normal, samp_tree_clamp, uv0);
    let n1 = textureSample(tree_normal, samp_tree_clamp, uv1);
    let n2 = textureSample(tree_normal, samp_tree_clamp, uv2);
    // Frame-local uvs outside 0..1 lie outside the captured sphere: no coverage.
    let inside = vec3<f32>(
        select(0.0, 1.0, all(in.f0.zw >= vec2<f32>(0.0)) && all(in.f0.zw <= vec2<f32>(1.0))),
        select(0.0, 1.0, all(in.f1.zw >= vec2<f32>(0.0)) && all(in.f1.zw <= vec2<f32>(1.0))),
        select(0.0, 1.0, all(in.f2.zw >= vec2<f32>(0.0)) && all(in.f2.zw <= vec2<f32>(1.0))));
    let w = in.w;
    let alpha = w.x * a0.a * inside.x + w.y * a1.a * inside.y + w.z * a2.a * inside.z;
    // The runtime's alpha-test reference (fAlphaRef 0.5; the baker keeps each frame's coverage at
    // it in every mip).
    var threshold = g.rt[0].w;
    if (g.params.z >= 0.0) {
        threshold = g.params.z;
    }
    if (alpha < threshold || bayer4(in.clip.xy) > in.fade) {
        discard;
    }
    // Colour: weights renormalised over the covering frames (an uncovered frame's dilated colour
    // would bleed in).
    let cw = w * vec3<f32>(a0.a, a1.a, a2.a) * inside;
    // Calibration (brightness * tint) applies to the encoded (gamma-space) albedo; the runtime
    // cannot apply it yet (CS's DistantTree pixel shader has no colour input), so it is off by
    // default.
    let tint = select(vec3<f32>(1.0), in.tint.rgb, g.rt[2].w > 0.5);
    let base = to_linear(to_gamma((cw.x * a0.rgb + cw.y * a1.rgb + cw.z * a2.rgb) / max(cw.x + cw.y + cw.z, 1e-4)) * tint);
    let prelit = in.tint.w > 0.5;
    // Normal: baked model-space normals, eased toward the canopy shell for stable far shading.
    let baked = normalize(cw.x * (n0.rgb * 2.0 - 1.0) + cw.y * (n1.rgb * 2.0 - 1.0) + cw.z * (n2.rgb * 2.0 - 1.0) + vec3<f32>(0.0, 0.0, 1e-4));
    let shell = normalize(w.x * imp_shell(in.f0, n0.a, grid) + w.y * imp_shell(in.f1, n1.a, grid) + w.z * imp_shell(in.f2, n2.a, grid) + vec3<f32>(0.0, 0.0, 1e-4));
    let n_model = normalize(mix(shell, baked, 0.6));
    let n = rot_z(n_model, -in.info.z);
    let mode = i32(g.params.x);
    var c = base;
    if (mode == 0 && prelit && atmo_on()) {
        // Pre-lit albedo: lit like a billboard, a vertical surface turned partly toward the sun
        // (as fs_tree, and as the game's DistantTree shader treats a flat quad).
        c = apply_fog(weather_shade(base, g.atmo[7].rgb + g.atmo[6].rgb * 0.5), in.world);
    } else if (mode == 0 && prelit) {
        let ambient = g.sun_dir.w;
        c = base * (ambient + (1.0 - ambient) * 0.6);
    } else if (mode == 0 && atmo_on()) {
        c = apply_fog(weather_shade(base, weather_light(n)), in.world);
    } else if (mode == 0) {
        c = base * lit(n);
    } else if (mode == 2) {
        c = n * 0.5 + 0.5;
    } else if (mode == 3) {
        c = vec3<f32>(0.75) * lit(n);
    } else if (mode == 4) {
        c = g.level_colors[0].rgb;
    } else if (mode == 5) {
        c = g.kind_colors[3].rgb;
    }
    return vec4<f32>(c, 1.0);
}

// Sky gradient: one full-screen triangle behind the scene.
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    var out: SkyOut;
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u)) * 2.0 - 1.0;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let h = g.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let dir = normalize(h.xyz / h.w - g.cam_pos.xyz);
    return vec4<f32>(sky_color(dir), 1.0);
}

// Overlay lines (grid, bounds, selection) in world space.
struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(in: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = g.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}
