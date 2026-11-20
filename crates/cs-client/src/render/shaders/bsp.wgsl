struct Uniforms {
    view_proj: mat4x4<f32>,
    // x = gamma, y = brightness, z = time_sec, w = debug_mode (0=normal, 1=lightmap, 2=fullbright)
    params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

@group(0) @binding(1)
var t_lightmap: texture_2d<f32>;
@group(0) @binding(2)
var s_lightmap: sampler;

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

const FLAG_ALPHATEST: u32 = 1u;
const FLAG_WATER: u32 = 2u;
const FLAG_NO_LIGHTMAP: u32 = 4u;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lm_uv: vec2<f32>,
    @location(4) alpha: f32,
    @location(5) flags: u32,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) @interpolate(flat) flags: u32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.alpha = in.alpha;
    out.flags = in.flags;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    if (in.flags & FLAG_WATER) != 0u {
        let t = u.params.z * 2.0;
        uv.x = uv.x + sin(uv.y * 6.28318 + t) * 0.04;
        uv.y = uv.y + cos(uv.x * 6.28318 + t) * 0.04;
    }

    let albedo = textureSample(t_diffuse, s_diffuse, uv);
    if (in.flags & FLAG_ALPHATEST) != 0u && albedo.a < 0.5 {
        discard;
    }

    let debug_mode = u.params.w;
    var lm = textureSample(t_lightmap, s_lightmap, in.lm_uv).rgb;
    if (in.flags & FLAG_NO_LIGHTMAP) != 0u || debug_mode > 1.5 {
        lm = vec3<f32>(0.5, 0.5, 0.5);
    }

    var lit: vec3<f32>;
    if debug_mode > 0.5 && debug_mode < 1.5 {
        // r_lightmap_only 1
        lit = lm * 2.0;
    } else {
        // Authentic GoldSrc overbright x2 lighting
        lit = albedo.rgb * lm * 2.0;
    }

    let brightness = max(u.params.y, 0.1);
    let gamma_val = max(u.params.x, 0.5);
    // GoldSrc baseline gamma is ~2.5; normalize relative to 2.5 so default looks crisp on modern sRGB displays
    let gamma_exp = 2.5 / gamma_val;
    let final_rgb = pow(clamp(lit * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));

    return vec4<f32>(final_rgb, albedo.a * in.alpha);
}
