struct MdlUniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,        // xyz = world camera position
    cam_right: vec4<f32>,         // xyz = camera right basis (for chrome)
    cam_up: vec4<f32>,            // xyz = camera up basis (for chrome)
    fog_color_density: vec4<f32>, // rgb = fog color, w = fog density
    gamma_params: vec4<f32>,      // x = gamma, y = texgamma, z = brightness, w = software_mode
    light_color: vec4<f32>,       // rgb = floor R_LightPoint + elights, a = alpha_test_flag
    render_color: vec4<f32>,      // rgb = entity rendercolor, a = entity blend/alpha
    flags_params: vec4<f32>,      // x = ent_mode (0=Normal,1=TransColor,2=TransAdd,3=TransAlpha,4=Blend,5=GlowShell), y = tex_flags, z = shell_offset, w = light_mul
};

struct LightsUniform {
    dlight_pos_rad: array<vec4<f32>, 16>,
    dlight_col: array<vec4<f32>, 16>,
    spot_pos_rad: array<vec4<f32>, 4>,
    spot_dir_cos: array<vec4<f32>, 4>,
    spot_col: array<vec4<f32>, 4>,
    counts: vec4<u32>,
};

@group(0) @binding(0)
var<uniform> u: MdlUniforms;
@group(0) @binding(1)
var<uniform> lights: LightsUniform;

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world_pos: vec3<f32>,
};

const TEX_FLAG_FLATSHADE: u32 = 1u;
const TEX_FLAG_CHROME: u32 = 2u;
const TEX_FLAG_ADDITIVE: u32 = 32u;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let n = normalize(in.normal);
    let wp = in.position + n * u.flags_params.z;
    out.clip_pos = u.view_proj * vec4<f32>(wp, 1.0);
    out.normal = n;
    let tex_flags = u32(u.flags_params.y);
    if (tex_flags & TEX_FLAG_CHROME) != 0u {
        let cu = dot(n, u.cam_right.xyz) * 0.5 + 0.5;
        let cv = -dot(n, u.cam_up.xyz) * 0.5 + 0.5;
        out.uv = vec2<f32>(cu, cv);
    } else {
        out.uv = in.uv;
    }
    out.world_pos = wp;
    return out;
}

fn bayer4x4(p: vec2<f32>) -> f32 {
    let x = i32(floor(p.x)) & 3;
    let y = i32(floor(p.y)) & 3;
    let m = array<f32, 16>(
        0.0,  8.0,  2.0, 10.0,
        12.0, 4.0, 14.0,  6.0,
        3.0, 11.0,  1.0,  9.0,
        15.0, 7.0, 13.0,  5.0
    );
    return m[y * 4 + x] / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let ent_mode = i32(u.flags_params.x);
    let tex_flags = u32(u.flags_params.y);

    if ent_mode == 5 {
        let view_dir = normalize(u.camera_pos.xyz - in.world_pos);
        let rim = pow(clamp(1.0 - abs(dot(normalize(in.normal), view_dir)), 0.0, 1.0), 1.4);
        let shell_rgb = u.render_color.rgb * (0.35 + 0.65 * rim);
        return vec4<f32>(shell_rgb, u.render_color.a);
    }

    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    if (u.light_color.a > 0.5 || ent_mode == 3) && albedo.a < 0.5 {
        discard;
    }

    let wn = normalize(in.normal);
    var total_light = u.light_color.rgb * max(u.flags_params.w, 0.0);
    if (tex_flags & TEX_FLAG_FLATSHADE) == 0u {
        let light_dir = normalize(vec3<f32>(0.4, 0.25, 0.88));
        let ndl = dot(wn, light_dir) * 0.35 + 0.72;
        total_light = total_light * ndl;
    }

    // Dynamic point lights + flashlight spotlights
    let nd = min(lights.counts.x, 16u);
    for (var i = 0u; i < nd; i = i + 1u) {
        let pr = lights.dlight_pos_rad[i];
        let delta = pr.xyz - in.world_pos;
        let dist = length(delta);
        if dist < pr.w && dist > 0.001 {
            let atten = 1.0 - dist / pr.w;
            let facing = max(dot(wn, delta / dist), 0.2);
            total_light = total_light + lights.dlight_col[i].rgb * (atten * facing * 0.75);
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
                let facing = max(dot(wn, -ldir), 0.2);
                total_light = total_light + lights.spot_col[j].rgb * (atten * facing * 0.85);
            }
        }
    }

    let tex_exp = max(u.gamma_params.y, 0.5) * 0.5;
    let albedo_lin = pow(clamp(albedo.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(tex_exp));
    var lit = albedo_lin * total_light;
    if (tex_flags & TEX_FLAG_ADDITIVE) != 0u || ent_mode == 2 {
        lit = albedo_lin * u.render_color.rgb;
    } else if ent_mode == 1 {
        lit = mix(lit, u.render_color.rgb * total_light, 0.75);
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

    let is_add = ent_mode == 2 || (tex_flags & TEX_FLAG_ADDITIVE) != 0u;
    if u.fog_color_density.w > 0.0 && !is_add {
        let cam_dist = length(in.world_pos - u.camera_pos.xyz);
        let fd = cam_dist * u.fog_color_density.w;
        let f = exp(-(fd * fd));
        final_rgb = mix(u.fog_color_density.rgb, final_rgb, clamp(f, 0.0, 1.0));
    }

    if is_add {
        return vec4<f32>(final_rgb, u.render_color.a);
    }
    if ent_mode == 1 {
        return vec4<f32>(final_rgb, u.render_color.a);
    }
    return vec4<f32>(final_rgb, albedo.a * u.render_color.a);
}
