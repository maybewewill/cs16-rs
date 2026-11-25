struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,        // xyz = camera world pos
    fog_color_density: vec4<f32>, // rgb = 0..1 fog color, w = fog density (0 = off)
    gamma_params: vec4<f32>,      // x = gamma, y = texgamma, z = brightness, w = software_mode
    misc_params: vec4<f32>,       // x = time_sec, y = debug_mode, z = wave_height, w = unused
};

struct LightsUniform {
    dlight_pos_rad: array<vec4<f32>, 16>,
    dlight_col: array<vec4<f32>, 16>,
    spot_pos_rad: array<vec4<f32>, 4>,
    spot_dir_cos: array<vec4<f32>, 4>,
    spot_col: array<vec4<f32>, 4>,
    counts: vec4<u32>, // x = num_dlights, y = num_spots
};

@group(0) @binding(0)
var<uniform> u: Uniforms;
@group(0) @binding(1)
var t_lightmap: texture_2d<f32>;
@group(0) @binding(2)
var s_lightmap: sampler;
@group(0) @binding(3)
var<uniform> lights: LightsUniform;

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

// Per-entity data (dynamic offset). Slot 0 is identity/mode 0, used by the world pass.
// params.x modes: 0 world, 1 TransColor, 2 TransAdd (blend ONE,ONE), 3 TransAlpha, 4 alpha blend.
// params.y: UV U-scroll offset (used by conveyor / scrolling textures).
// color.rgb = rendercolor, color.a = entity blend.
struct EntityUniforms {
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: vec4<f32>,
};

@group(2) @binding(0)
var<uniform> ent: EntityUniforms;

const FLAG_ALPHATEST: u32 = 1u;
const FLAG_WATER: u32 = 2u;
const FLAG_NO_LIGHTMAP: u32 = 4u;
const FLAG_SCROLL: u32 = 8u;

// GoldSrc / Xash3D FWGS `TURBSCALE` and 256-entry `r_turbsin` (`engine/warpsin.h`, `ref/gl/gl_rsurf.c`).
const TURBSCALE: f32 = 40.74366543;
const R_TURBSIN: array<f32, 256> = array<f32, 256>(
    0.000000, 0.098165, 0.196270, 0.294259, 0.392069, 0.489643, 0.586920, 0.683850,
    0.780360, 0.876405, 0.971920, 1.066850, 1.161140, 1.254725, 1.347560, 1.439580,
    1.530735, 1.620965, 1.710220, 1.798445, 1.885585, 1.971595, 2.056410, 2.139990,
    2.222280, 2.303235, 2.382795, 2.460925, 2.537575, 2.612690, 2.686235, 2.758160,
    2.828425, 2.896990, 2.963805, 3.028835, 3.092040, 3.153385, 3.212830, 3.270340,
    3.325880, 3.379415, 3.430915, 3.480350, 3.527685, 3.572895, 3.615955, 3.656840,
    3.695520, 3.731970, 3.766175, 3.798115, 3.827760, 3.855105, 3.880125, 3.902810,
    3.923140, 3.941110, 3.956705, 3.969920, 3.980740, 3.989160, 3.995180, 3.998795,
    4.000000, 3.998795, 3.995180, 3.989160, 3.980740, 3.969920, 3.956705, 3.941110,
    3.923140, 3.902810, 3.880125, 3.855105, 3.827760, 3.798115, 3.766175, 3.731970,
    3.695520, 3.656840, 3.615955, 3.572895, 3.527685, 3.480350, 3.430915, 3.379415,
    3.325880, 3.270340, 3.212830, 3.153385, 3.092040, 3.028835, 2.963805, 2.896990,
    2.828425, 2.758160, 2.686235, 2.612690, 2.537575, 2.460925, 2.382795, 2.303235,
    2.222280, 2.139990, 2.056410, 1.971595, 1.885585, 1.798445, 1.710220, 1.620965,
    1.530735, 1.439580, 1.347560, 1.254725, 1.161140, 1.066850, 0.971920, 0.876405,
    0.780360, 0.683850, 0.586920, 0.489643, 0.392069, 0.294259, 0.196270, 0.098165,
    0.000000, -0.098165, -0.196270, -0.294259, -0.392069, -0.489643, -0.586920, -0.683850,
    -0.780360, -0.876405, -0.971920, -1.066850, -1.161140, -1.254725, -1.347560, -1.439580,
    -1.530735, -1.620965, -1.710220, -1.798445, -1.885585, -1.971595, -2.056410, -2.139990,
    -2.222280, -2.303235, -2.382795, -2.460925, -2.537575, -2.612690, -2.686235, -2.758160,
    -2.828425, -2.896990, -2.963805, -3.028835, -3.092040, -3.153385, -3.212830, -3.270340,
    -3.325880, -3.379415, -3.430915, -3.480350, -3.527685, -3.572895, -3.615955, -3.656840,
    -3.695520, -3.731970, -3.766175, -3.798115, -3.827760, -3.855105, -3.880125, -3.902810,
    -3.923140, -3.941110, -3.956705, -3.969920, -3.980740, -3.989160, -3.995180, -3.998795,
    -4.000000, -3.998795, -3.995180, -3.989160, -3.980740, -3.969920, -3.956705, -3.941110,
    -3.923140, -3.902810, -3.880125, -3.855105, -3.827760, -3.798115, -3.766175, -3.731970,
    -3.695520, -3.656840, -3.615955, -3.572895, -3.527685, -3.480350, -3.430915, -3.379415,
    -3.325880, -3.270340, -3.212830, -3.153385, -3.092040, -3.028835, -2.963805, -2.896990,
    -2.828425, -2.758160, -2.686235, -2.612690, -2.537575, -2.460925, -2.382795, -2.303235,
    -2.222280, -2.139990, -2.056410, -1.971595, -1.885585, -1.798445, -1.710220, -1.620965,
    -1.530735, -1.439580, -1.347560, -1.254725, -1.161140, -1.066850, -0.971920, -0.876405,
    -0.780360, -0.683850, -0.586920, -0.489643, -0.392069, -0.294259, -0.196270, -0.098165
);

fn r_turbsin_at(v: f32) -> f32 {
    // Convert via signed i32 first so negative inputs wrap modulo 256 like C `(int)v & 255`.
    let idx = u32(i32(v) & 255);
    return R_TURBSIN[idx];
}

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
    @location(4) world_pos: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    var local_pos = in.position;
    if (in.flags & FLAG_WATER) != 0u && u.misc_params.z != 0.0 {
        let wp_z = (ent.model * vec4<f32>(local_pos, 1.0)).z;
        let wh = select(u.misc_params.z, -u.misc_params.z, wp_z >= u.camera_pos.z);
        let t = u.misc_params.x;
        let nv1 = r_turbsin_at(t * 160.0 + local_pos.y + local_pos.x) + 8.0;
        let nv2 = (r_turbsin_at(local_pos.x * 5.0 + t * 171.0 - local_pos.y) + 8.0) * 0.8 + nv1;
        local_pos.z = local_pos.z + nv2 * wh;
    }
    let wp = (ent.model * vec4<f32>(local_pos, 1.0)).xyz;
    let wn = normalize((ent.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.clip_pos = u.view_proj * vec4<f32>(wp, 1.0);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.alpha = in.alpha;
    out.flags = in.flags;
    out.world_pos = wp;
    out.world_normal = wn;
    return out;
}

fn bayer4x4(p: vec2<f32>) -> f32 {
    let x = i32(floor(p.x)) & 3;
    let y = i32(floor(p.y)) & 3;
    let idx = y * 4 + x;
    let m = array<f32, 16>(
        0.0,  8.0,  2.0, 10.0,
        12.0, 4.0, 14.0,  6.0,
        3.0, 11.0,  1.0,  9.0,
        15.0, 7.0, 13.0,  5.0
    );
    return m[idx] / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    let t = u.misc_params.x;
    if (in.flags & FLAG_SCROLL) != 0u {
        let scroll = select(-t * 0.4, ent.params.y, abs(ent.params.y) > 0.0001);
        uv.x = uv.x + scroll;
    }
    if (in.flags & FLAG_WATER) != 0u {
        let tex_size = vec2<f32>(textureDimensions(t_diffuse));
        let os = uv.x * tex_size.x;
        let ot = uv.y * tex_size.y;
        let s = os + r_turbsin_at((ot * 0.125 + t) * TURBSCALE);
        let t_warped = ot + r_turbsin_at((os * 0.125 + t) * TURBSCALE);
        uv = vec2<f32>(s / max(tex_size.x, 1.0), t_warped / max(tex_size.y, 1.0));
    }

    let albedo = textureSample(t_diffuse, s_diffuse, uv);
    let ent_mode = i32(ent.params.x);
    if (in.flags & FLAG_ALPHATEST) != 0u && albedo.a < 0.5 {
        discard;
    }
    if ent_mode == 3 && albedo.a < 0.25 {
        discard;
    }

    let debug_mode = u.misc_params.y;
    let overbright_scale = select(1.0, 2.0, u.gamma_params.x > 0.5);
    var lm = textureSample(t_lightmap, s_lightmap, in.lm_uv).rgb;
    if (in.flags & FLAG_NO_LIGHTMAP) != 0u || debug_mode > 1.5 {
        lm = vec3<f32>(select(1.0, 0.5, u.gamma_params.x > 0.5));
    }

    var lit: vec3<f32>;
    if debug_mode > 0.5 && debug_mode < 1.5 {
        lit = lm * overbright_scale;
    } else {
        lit = albedo.rgb * lm * overbright_scale;
    }

    var final_rgb = clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0));

    if u.gamma_params.w > 0.5 {
        let d = bayer4x4(in.clip_pos.xy);
        let steps = vec3<f32>(31.0, 63.0, 31.0);
        final_rgb = clamp(floor(final_rgb * steps + vec3<f32>(d)) / steps, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if u.fog_color_density.w > 0.0 && ent_mode != 2 {
        let cam_dist = length(in.world_pos - u.camera_pos.xyz);
        let fd = cam_dist * u.fog_color_density.w;
        let f = exp(-(fd * fd));
        final_rgb = mix(u.fog_color_density.rgb, final_rgb, clamp(f, 0.0, 1.0));
    }

    if ent_mode == 1 {
        return vec4<f32>(ent.color.rgb, ent.color.a);
    }
    if ent_mode == 2 {
        return vec4<f32>(final_rgb * ent.color.a, 1.0);
    }
    if ent_mode == 4 {
        return vec4<f32>(final_rgb, albedo.a * in.alpha * ent.color.a);
    }
    return vec4<f32>(final_rgb, albedo.a * in.alpha);
}
