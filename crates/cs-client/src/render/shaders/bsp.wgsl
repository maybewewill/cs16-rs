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
    if (in.flags & FLAG_WATER) != 0u && u.misc_params.z > 0.0 {
        let t = u.misc_params.x * 2.5;
        let wave = sin(local_pos.x * 0.05 + t) * cos(local_pos.y * 0.05 + t);
        local_pos.z = local_pos.z + wave * u.misc_params.z;
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
        let wt = t * 2.0;
        uv.x = uv.x + sin(uv.y * 6.28318 + wt) * 0.04;
        uv.y = uv.y + cos(uv.x * 6.28318 + wt) * 0.04;
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
    var lm = textureSample(t_lightmap, s_lightmap, in.lm_uv).rgb;
    if (in.flags & FLAG_NO_LIGHTMAP) != 0u || debug_mode > 1.5 {
        lm = vec3<f32>(0.5, 0.5, 0.5);
    }

    if debug_mode < 1.5 {
        let wn = normalize(in.world_normal);
        let nd = min(lights.counts.x, 16u);
        for (var i = 0u; i < nd; i = i + 1u) {
            let pr = lights.dlight_pos_rad[i];
            let delta = pr.xyz - in.world_pos;
            let dist = length(delta);
            if dist < pr.w && dist > 0.001 {
                let atten = 1.0 - dist / pr.w;
                let facing = max(dot(wn, delta / dist), 0.15);
                lm = lm + lights.dlight_col[i].rgb * (atten * facing * 0.5);
            }
        }
        let ns = min(lights.counts.y, 4u);
        for (var j = 0u; j < ns; j = j + 1u) {
            let sr = lights.spot_pos_rad[j];
            let dc = lights.spot_dir_cos[j];
            let delta = in.world_pos - sr.xyz;
            let dist = length(delta);
            if dist < sr.w && dist > 0.001 {
                let ldir = delta / dist;
                let c = dot(ldir, dc.xyz);
                if c > dc.w {
                    let cone = clamp((c - dc.w) / max(1.0 - dc.w, 0.01), 0.0, 1.0);
                    let atten = (1.0 - dist / sr.w) * cone;
                    let facing = max(dot(wn, -ldir), 0.15);
                    lm = lm + lights.spot_col[j].rgb * (atten * facing * 0.55);
                }
            }
        }
    }

    let tex_exp = max(u.gamma_params.y, 0.5) * 0.5;
    let albedo_lin = pow(clamp(albedo.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(tex_exp));

    var lit: vec3<f32>;
    if debug_mode > 0.5 && debug_mode < 1.5 {
        lit = lm * 2.0;
    } else {
        lit = albedo_lin * lm * 2.0;
    }

    let brightness = max(u.gamma_params.z, 0.1);
    let gamma_val = max(u.gamma_params.x, 0.5);
    let gamma_exp = 2.5 / gamma_val;
    var final_rgb = pow(clamp(lit * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));

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
