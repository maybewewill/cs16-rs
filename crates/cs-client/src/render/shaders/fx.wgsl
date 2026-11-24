struct FxUniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    fog_color_density: vec4<f32>,
    gamma_params: vec4<f32>, // gamma, texgamma, brightness, software_mode
};

@group(0) @binding(0)
var<uniform> u: FxUniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    out.world_pos = in.position;
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
    let brightness = max(u.gamma_params.z, 0.1);
    let gamma_val = max(u.gamma_params.x, 0.5);
    let gamma_exp = 2.5 / gamma_val;
    var rgb = pow(clamp(in.color.rgb * brightness, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(gamma_exp));

    if u.gamma_params.w > 0.5 {
        let d = bayer4x4(in.clip_pos.xy);
        let steps = vec3<f32>(31.0, 63.0, 31.0);
        rgb = clamp(floor(rgb * steps + vec3<f32>(d)) / steps, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if u.fog_color_density.w > 0.0 {
        let cam_dist = length(in.world_pos - u.camera_pos.xyz);
        let fd = cam_dist * u.fog_color_density.w;
        let f = exp(-(fd * fd));
        rgb = mix(u.fog_color_density.rgb, rgb, clamp(f, 0.0, 1.0));
    }

    return vec4<f32>(rgb, in.color.a);
}
