// Billboard shader of lodforge-texgen: an orthographic render of one mesh of a tree or grass
// model, either lit (the diffuse billboard) or as a normal map of the view (the `_n` texture).
//
// Written 2026-10-19 from a functional description of what the output must hold (lit colour per
// texel, view-space normal encoding, material terms) and the uniform / vertex layout of
// render.rs, without reference to any other billboard shader. Where the description leaves a
// detail open, the plain physical reading was tried first and the golden TexGen billboards
// arbitrated (docs/baseline.md, "Billboard shader re-derivation"); each such choice is noted at
// the place it is made.
//
// Output conventions (render.rs): the target is RGBA8 without blending, drawn with MSAA, so the
// resolved alpha is the sample coverage and the resolved colour is coverage-weighted. Every
// covered fragment therefore writes alpha 1 (both passes) and the post-process un-premultiplies.

struct Uniforms {
    // Rows of the model -> clip transform: clip.x = dot(rx.xyz, p) + rx.w, likewise y and z.
    rx: vec4<f32>,
    ry: vec4<f32>,
    rz: vec4<f32>,
    // xyz: direction toward the light (model space, unit); w: ambient intensity.
    light: vec4<f32>,
    // xyz: direction toward the camera (model space, unit); w: direct intensity.
    eye: vec4<f32>,
    // x: alpha-test threshold in 0..1 (negative: no test); y: pass (0 diffuse, 1 normal);
    // z: brightness scale of PBR materials; w: material flags (FLAG_*).
    params: vec4<f32>,
    // rgb: specular colour; w: specular strength.
    spec: vec4<f32>,
    // x: glossiness (Blinn-Phong exponent); y: lighting effect 1; z: lighting effect 2 (rim
    // power); w: normal pass pull toward the viewer (NORMAL_VIEW_PULL for billboards).
    spec2: vec4<f32>,
    // rgb: own-emit colour times its multiple; black when the material does not own-emit.
    emit: vec4<f32>,
    // Object normal + depth pass (pass 2): x = dot(p, toward camera) on the slab's near plane,
    // y = 1 / slab length; z: mip bias of every texture sample (0 for billboards).
    slab: vec4<f32>,
};

// Material flags in `params.w` (render.rs `flag`).
const FLAG_SPECULAR: u32 = 1u;
const FLAG_SOFT: u32 = 2u;
const FLAG_RIM: u32 = 4u;
const FLAG_GLOW_MAP: u32 = 8u;
const FLAG_TANGENTS: u32 = 16u;
const FLAG_PBR: u32 = 32u;
const FLAG_COMPLEX: u32 = 64u;

// Soft-light fill as a fraction of the direct intensity (fitted on the golden tree billboards:
// 0.7..0.85 score alike, 1.0 costs 0.3 dB, 0.5 costs 0.4 dB).
const SOFT_FILL: f32 = 0.75;

// Normal pass: the encoded direction is the surface normal (made to face the viewer) pulled
// toward the viewer by this much, in units of a unit normal; 1.0 is the half-way direction
// between normal and view. The description asks for the surface normal itself (0.0), but the
// golden normal textures are nearly frontal (blue mean 249 on a bare trunk cylinder) with the
// surface tilt at reduced strength, and 1.0 scores 20.1 / 22.2 dB on trunks / HD grass against
// 18.6 / 12.2 dB for the surface normal.
const NORMAL_VIEW_PULL: f32 = 1.0;

@group(0) @binding(0) var<uniform> u: Uniforms;
// Diffuse (albedo + alpha); for complex grass the diffuse in the top half, the normal map in the
// bottom half.
@group(0) @binding(1) var diffuse_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
// Tangent-space normal map (flat when the shape has none); alpha = specular mask.
@group(0) @binding(3) var normal_tex: texture_2d<f32>;
// Texture-set slot 2: glow map, or the soft / rim light mask (black when the shape has none).
@group(0) @binding(4) var glow_tex: texture_2d<f32>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    // The NIF's tangent frame, model space: `tangent` runs along texture v, `bitangent` along u
    // (zero vectors when the shape has no tangents; see FLAG_TANGENTS).
    @location(3) tangent: vec3<f32>,
    @location(4) bitangent: vec3<f32>,
    @location(5) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec3<f32>,
    @location(3) bitangent: vec3<f32>,
    @location(4) color: vec4<f32>,
    // Model-space position (slab depth of pass 2).
    @location(5) mpos: vec3<f32>,
};

@vertex
fn vs(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(dot(u.rx.xyz, in.pos) + u.rx.w, dot(u.ry.xyz, in.pos) + u.ry.w, dot(u.rz.xyz, in.pos) + u.rz.w, 1.0);
    out.uv = in.uv;
    out.normal = in.normal;
    out.tangent = in.tangent;
    out.bitangent = in.bitangent;
    out.color = in.color;
    out.mpos = in.pos;
    return out;
}

fn has(f: u32) -> bool {
    return (u32(u.params.w) & f) != 0u;
}

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    return select(v / l, vec3<f32>(0.0, 0.0, 1.0), l < 1e-8);
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let complex = has(FLAG_COMPLEX);

    // Complex grass: one texture, diffuse above (v in 0..0.5), normal map below (0.5..1). The
    // mesh's own v is clamped to 0..1 so neither half samples into the other.
    var uv_diffuse = in.uv;
    var uv_normal = in.uv;
    if complex {
        let v = clamp(in.uv.y, 0.0, 1.0);
        uv_diffuse = vec2<f32>(in.uv.x, v * 0.5);
        uv_normal = vec2<f32>(in.uv.x, 0.5 + v * 0.5);
    }
    // All texture reads happen before the alpha test so they stay in uniform control flow.
    let albedo = textureSampleBias(diffuse_tex, samp, uv_diffuse, u.slab.z);
    let nm_own = textureSampleBias(normal_tex, samp, uv_normal, u.slab.z);
    let nm_complex = textureSampleBias(diffuse_tex, samp, uv_normal, u.slab.z);
    let mask = textureSampleBias(glow_tex, samp, uv_diffuse, u.slab.z);
    let nm = select(nm_own, nm_complex, complex);

    // Coverage: the alpha test on the diffuse alpha. A texel passes when its alpha exceeds the
    // threshold (so a threshold of 0 still drops fully transparent texels).
    if u.params.x >= 0.0 && albedo.a <= u.params.x {
        discard;
    }

    // Shading normal, model space: the normal map's tangent-space normal (x along u = the stored
    // bitangent, y along v = the stored tangent, z = the vertex normal), or the interpolated vertex
    // normal when the shape has no tangent frame. The map's green is taken as pointing along +v
    // (the convention of the game's normal maps; the opposite sign costs 1.8 dB on trees).
    let n_vertex = safe_normalize(in.normal);
    var n = n_vertex;
    if has(FLAG_TANGENTS) {
        let t = nm.xyz * 2.0 - 1.0;
        n = safe_normalize(t.x * safe_normalize(in.bitangent) + t.y * safe_normalize(in.tangent) + t.z * n_vertex);
    }
    // Two-sidedness: the normal is used as stored on both faces. Double-sided shapes here are
    // foliage cards, lit through rather than as opaque backs (flipping on back faces costs 2.6 dB).

    let to_light = safe_normalize(u.light.xyz);
    let to_eye = safe_normalize(u.eye.xyz);

    if u.params.y > 1.5 {
        // Object normal + depth pass (impostors, RGBA16F target): the shading normal in model
        // space, turned to the viewer's side on back faces of two-sided shapes (what is seen of
        // a leaf card is its visible side), and the depth across the slab, 0 at the near plane.
        // Never the zero vector, which the readback takes as "not covered".
        let n_os = select(-n, n, front);
        let depth = (u.slab.x - dot(in.mpos, to_eye)) * u.slab.y;
        return vec4<f32>(n_os, clamp(depth, 0.0, 1.0));
    }
    if u.params.y > 0.5 {
        // Normal pass: n in the view basis (right, up, toward the viewer). A normal facing away
        // from the viewer keeps its tilt and only its depth component is turned around (negating
        // the whole vector mirrors the map's detail and costs 7 dB on HD grass); then the pull
        // toward the viewer. Encoded like the game's normal maps: red = right, green = down
        // (0.5 - 0.5 up; green = up costs 2.5 dB on trunks), blue = toward the viewer.
        let right = safe_normalize(cross(vec3<f32>(0.0, 0.0, 1.0), to_eye));
        let up = cross(to_eye, right);
        let nv = safe_normalize(vec3<f32>(dot(n, right), dot(n, up), abs(dot(n, to_eye)) + u.spec2.w));
        return vec4<f32>(0.5 + 0.5 * nv.x, 0.5 - 0.5 * nv.y, 0.5 + 0.5 * nv.z, 1.0);
    }

    // Diffuse pass.
    let ambient = u.light.w;
    let direct = u.eye.w;
    let n_dot_l = dot(n, to_light);
    let lit = max(n_dot_l, 0.0);
    let base = in.color.rgb * albedo.rgb;
    var color = base * (ambient + direct * lit);

    // Soft lighting: thin foliage is lit from the directions the direct light does not reach,
    // in proportion to how far the surface turns away from it, through the slot-2 mask and in the
    // surface colour. The material's lighting effect 1 ("subsurface rolloff", 1..10 on the
    // vanilla trees) is not used: every scaling or exponent tried with it scored lower than this
    // constant fill (wrapped-diffuse, back-lit and constant forms: 23.0..24.0 dB against 25.3).
    if has(FLAG_SOFT) {
        color += base * mask.rgb * direct * SOFT_FILL * (1.0 - lit);
    }
    // Rim lighting: brightest where the surface grazes the view, falling off with the power in
    // lighting effect 2, through the slot-2 mask; independent of the light direction. (No vanilla
    // tree uses it, so the golden billboards do not constrain it.)
    if has(FLAG_RIM) {
        let grazing = 1.0 - clamp(dot(n, to_eye), 0.0, 1.0);
        color += mask.rgb * direct * pow(grazing, max(u.spec2.z, 1e-3));
    }
    // Glow map: own-emit colour through the map, added to the light; not scaled by the vertex
    // colour (emission is not reflected light).
    if has(FLAG_GLOW_MAP) {
        color += mask.rgb * u.emit.rgb;
    }
    // Blinn-Phong specular of non-PBR specular materials, masked by the normal map's alpha.
    if has(FLAG_SPECULAR) && !has(FLAG_PBR) {
        let half_vec = safe_normalize(to_light + to_eye);
        let n_dot_h = max(dot(n, half_vec), 0.0);
        color += u.spec.rgb * u.spec.w * nm.a * direct * pow(n_dot_h, max(u.spec2.x, 1.0));
    }
    // PBR materials are rendered darker (the job's brightness scale).
    if has(FLAG_PBR) {
        color *= u.params.z;
    }
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
