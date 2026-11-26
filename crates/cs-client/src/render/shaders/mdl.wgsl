struct MdlUniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,          // xyz = world camera position
    cam_right: vec4<f32>,           // xyz = camera right basis (for chrome)
    cam_up: vec4<f32>,              // xyz = camera up basis (for chrome)
    fog_color_density: vec4<f32>,   // rgb = fog color, w = fog density
    gamma_params: vec4<f32>,        // x = gamma, y = texgamma, z = brightness, w = software_mode
    light_color: vec4<f32>,         // rgb = normalized entity lightcolor, a = alpha_test_flag
    render_color: vec4<f32>,        // rgb = entity rendercolor, a = entity blend/alpha
    flags_params: vec4<f32>,        // x = ent_mode (0=Normal,1=TransColor,2=TransAdd,3=TransAlpha,4=Blend,5=GlowShell,6=XRay), y = tex_flags, z = shell_offset, w = light_mul
    studio_light_vec: vec4<f32>,    // xyz = plightvec (normalized), w = ambientlight
    studio_light_params: vec4<f32>, // x = shadelight
};

// Packed 1024-entry GoldSrc lightgammatable (2 x u16 per u32 -> 128 vec4<u32> = 2,048 bytes).
struct LightGammaUniform {
    entries: array<vec4<u32>, 128>,
};

// Up to 128 posed 3x4 affine bone matrices (row-major: 3 vec4<f32> per bone = 384 vec4<f32> = 6,144 bytes).
// Strictly <= 16,384 bytes WebGL2 downlevel uniform buffer limit.
struct MdlBonesUniform {
    rows: array<vec4<f32>, 384>,
};

@group(0) @binding(0)
var<uniform> u: MdlUniforms;
@group(0) @binding(1)
var<uniform> light_gamma: LightGammaUniform;

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

@group(2) @binding(0)
var<uniform> bones: MdlBonesUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) pos_bone: u32,
    @location(4) norm_bone: u32,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) color: vec3<f32>,
};

const TEX_FLAG_FLATSHADE: u32 = 1u;
const TEX_FLAG_CHROME: u32 = 2u;
const TEX_FLAG_ADDITIVE: u32 = 32u;
const TEX_FLAG_FULLBRIGHT: u32 = 64u;
const SHADE_LAMBERT: f32 = 1.4953241;

fn normalize_or_zero(v: vec3<f32>) -> vec3<f32> {
    let len_sq = dot(v, v);
    if len_sq > 1e-12 {
        return v * inverseSqrt(len_sq);
    }
    return vec3<f32>(0.0);
}

fn skin_point(bone_idx: u32, p: vec3<f32>) -> vec3<f32> {
    let base = min(bone_idx, 127u) * 3u;
    let p4 = vec4<f32>(p, 1.0);
    return vec3<f32>(
        dot(bones.rows[base], p4),
        dot(bones.rows[base + 1u], p4),
        dot(bones.rows[base + 2u], p4),
    );
}

fn skin_vector(bone_idx: u32, v: vec3<f32>) -> vec3<f32> {
    let base = min(bone_idx, 127u) * 3u;
    return vec3<f32>(
        dot(bones.rows[base].xyz, v),
        dot(bones.rows[base + 1u].xyz, v),
        dot(bones.rows[base + 2u].xyz, v),
    );
}

fn lookup_light_gamma(b_10bit: u32) -> f32 {
    let idx = min(b_10bit, 1023u);
    let v = light_gamma.entries[idx >> 3u];
    let comp = (idx >> 1u) & 3u;
    var packed: u32 = v.x;
    if comp == 1u {
        packed = v.y;
    } else if comp == 2u {
        packed = v.z;
    } else if comp == 3u {
        packed = v.w;
    }
    let val = select(packed & 0xFFFFu, packed >> 16u, (idx & 1u) != 0u);
    return f32(val) / 1023.0;
}

fn r_studio_lighting(tex_flags: u32, normal: vec3<f32>) -> f32 {
    if (tex_flags & TEX_FLAG_FULLBRIGHT) != 0u {
        return 1.0;
    }
    let ambientlight = u.studio_light_vec.w;
    let shadelight = u.studio_light_params.x;
    let plightvec = u.studio_light_vec.xyz;
    var illum: f32;
    if (tex_flags & TEX_FLAG_FLATSHADE) != 0u {
        illum = ambientlight + shadelight * 0.8;
    } else {
        var lum = ambientlight + shadelight;
        let lightcos = min(dot(normal, plightvec), 1.0);
        let adj = (lightcos + (SHADE_LAMBERT - 1.0)) / SHADE_LAMBERT;
        if adj > 0.0 {
            lum = lum - shadelight * adj;
        }
        illum = max(lum, 0.0);
    }
    let clamped = clamp(illum, 0.0, 255.0);
    return lookup_light_gamma(u32(clamped * 4.0));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let skinned_pos = skin_point(in.pos_bone, in.position);
    let n = normalize_or_zero(skin_vector(in.norm_bone, in.normal));
    let wp = skinned_pos + n * u.flags_params.z;
    out.clip_pos = u.view_proj * vec4<f32>(wp, 1.0);
    out.normal = n;
    let tex_flags = u32(u.flags_params.y + 0.5);
    let lv = r_studio_lighting(tex_flags, n);
    out.color = u.light_color.rgb * lv;
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
        15.0, 7.0, 13.0,  5.0,
    );
    return m[y * 4 + x] / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let ent_mode = i32(u.flags_params.x + 0.5);
    let tex_flags = u32(u.flags_params.y + 0.5);

    if ent_mode == 5 {
        let view_dir = normalize_or_zero(u.camera_pos.xyz - in.world_pos);
        let rim = pow(clamp(1.0 - abs(dot(normalize_or_zero(in.normal), view_dir)), 0.0, 1.0), 1.4);
        let shell_rgb = u.render_color.rgb * (0.35 + 0.65 * rim);
        return vec4<f32>(shell_rgb, u.render_color.a);
    }

    if ent_mode == 6 {
        let view_dir = normalize_or_zero(u.camera_pos.xyz - in.world_pos);
        let rim = pow(clamp(1.0 - abs(dot(normalize_or_zero(in.normal), view_dir)), 0.0, 1.0), 1.35);
        let xray_rgb = u.render_color.rgb * (0.45 + 0.85 * rim);
        let xray_alpha = clamp(u.render_color.a * (0.40 + 0.60 * rim), 0.0, 1.0);
        return vec4<f32>(xray_rgb, xray_alpha);
    }

    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    if (u.light_color.a > 0.5 || ent_mode == 3) && albedo.a < 0.5 {
        discard;
    }

    let total_light = in.color;
    var lit = albedo.rgb * total_light;
    if (tex_flags & TEX_FLAG_ADDITIVE) != 0u || ent_mode == 2 {
        lit = albedo.rgb * u.render_color.rgb;
    } else if ent_mode == 1 {
        lit = mix(lit, u.render_color.rgb * total_light, 0.75);
    }

    var final_rgb = clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0));

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
