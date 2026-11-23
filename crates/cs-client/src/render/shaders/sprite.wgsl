struct Uniforms {
    view_proj: mat4x4<f32>,
    // x = gamma, y = brightness
    params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

@group(1) @binding(0)
var t_frame: texture_2d<f32>;
@group(1) @binding(1)
var s_frame: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_frame, s_frame, in.uv);
    // GL_ALPHA_TEST is on for every sprite (R_DrawSpriteModel): fully transparent texels never write.
    if tex.a < 0.004 {
        discard;
    }
    let lit = tex.rgb * in.color.rgb;
    let brightness = max(u.params.y, 0.1);
    let gamma_val = max(u.params.x, 0.5);
    let gamma_exp = 2.5 / gamma_val;
    let rgb = pow(clamp(lit * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));
    return vec4<f32>(rgb, tex.a * in.color.a);
}
