struct SkyUniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // x = gamma, y = brightness
};

@group(0) @binding(0)
var<uniform> u: SkyUniforms;

@group(1) @binding(0)
var t_face: texture_2d<f32>;
@group(1) @binding(1)
var s_face: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = u.view_proj * vec4<f32>(in.position, 1.0);
    // Force depth = 1.0 (far plane) in wgpu [0, 1] NDC so skybox sits cleanly behind all geometry
    out.clip_pos = vec4<f32>(p.xy, p.w * 0.99999, p.w);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sample_rgb = textureSample(t_face, s_face, in.uv).rgb;
    let brightness = max(u.params.y, 0.1);
    let gamma_val = max(u.params.x, 0.5);
    let gamma_exp = 2.5 / gamma_val;
    let rgb = pow(clamp(sample_rgb * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));
    return vec4<f32>(rgb, 1.0);
}
