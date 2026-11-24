struct SkyUniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>,            // x = gamma, y = texgamma, z = brightness, w = software_mode
    fog_color_density: vec4<f32>, // rgb = 0..1 fog color, w = fog density
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
    @location(1) world_dir: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = u.view_proj * vec4<f32>(in.position, 1.0);
    out.clip_pos = vec4<f32>(p.xy, p.w * 0.99999, p.w);
    out.uv = in.uv;
    out.world_dir = in.position;
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
    let sample_rgb = textureSample(t_face, s_face, in.uv).rgb;
    var rgb = clamp(sample_rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if u.params.w > 0.5 {
        let d = bayer4x4(in.clip_pos.xy);
        let steps = vec3<f32>(31.0, 63.0, 31.0);
        rgb = clamp(floor(rgb * steps + vec3<f32>(d)) / steps, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if u.fog_color_density.w > 0.0 {
        let horizon = 1.0 - abs(normalize(in.world_dir).z);
        let f = clamp(u.fog_color_density.w * 2048.0 * horizon, 0.0, 0.9);
        rgb = mix(rgb, u.fog_color_density.rgb, f);
    }
    return vec4<f32>(rgb, 1.0);
}
