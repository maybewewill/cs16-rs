struct MdlUniforms {
    view_proj: mat4x4<f32>,
    light_color: vec4<f32>, // rgb = BSP R_LightPoint ambient, a = gamma
    params: vec4<f32>,      // x = brightness, y = alpha_test_flag
};

@group(0) @binding(0)
var<uniform> u: MdlUniforms;

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
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    if u.params.y > 0.5 && albedo.a < 0.5 {
        discard;
    }

    // Classic GoldSrc StudioModel lambertian + ambient shade (keeps player silhouettes crisp & readable!)
    let light_dir = normalize(vec3<f32>(0.4, 0.25, 0.88));
    let ndl = dot(normalize(in.normal), light_dir) * 0.35 + 0.72;
    let lit = albedo.rgb * u.light_color.rgb * ndl;

    let brightness = max(u.params.x, 0.1);
    let gamma_val = max(u.light_color.a, 0.5);
    let gamma_exp = 2.5 / gamma_val;
    let rgb = pow(clamp(lit * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));
    return vec4<f32>(rgb, albedo.a);
}
