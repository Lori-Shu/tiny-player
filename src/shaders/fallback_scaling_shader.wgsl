
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;


    let x = f32(i32(in_vertex_index) / 2 % 2) * 2.0 - 1.0;
    let y = f32(i32(in_vertex_index) % 2) * 2.0 - 1.0;


    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0)
    );

    let current_pos = pos[in_vertex_index];
    out.clip_position = vec4<f32>(current_pos, 0.0, 1.0);


    out.tex_coords = vec2<f32>(
        current_pos.x * 0.5 + 0.5,
        1.0 - (current_pos.y * 0.5 + 0.5)
    );

    return out;
}
struct ColorSpaceUniform {
    yuv2rgb_matrix: mat4x4<f32>,
    yuv_offset: vec4<f32>,
}
struct HDRFlagUniform {
    flag: vec4<u32>,
}

@group(1) @binding(0) var<uniform> cs_params: ColorSpaceUniform;
@group(1) @binding(1) var<uniform> hdr_flag: HDRFlagUniform;

@group(0) @binding(0) var t_y: texture_2d<f32>;
@group(0) @binding(1) var t_u: texture_2d<f32>;
@group(0) @binding(2) var t_v: texture_2d<f32>;
@group(0) @binding(3) var s_sampler: sampler;
fn pq_to_linear(color: vec3<f32>) -> vec3<f32> {
    const m1 = 1305.0 / 8192.0;
    const m2 = 2523.0 / 32.0;
    const c1 = 107.0 / 128.0;
    const c2 = (2413.0 / 4096.0) * 32.0;
    const c3 = (2392.0 / 4096.0) * 32.0;

    let safe_color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    let p_pow = pow(safe_color, vec3<f32>(1.0 / m2));
    let num = max(p_pow - c1, vec3<f32>(0.0));
    let den = c2 - (c3 * p_pow);
    return pow(num / den, vec3<f32>(1.0 / m1));
}

fn bt2020_to_bt709(color: vec3<f32>) -> vec3<f32> {

let m = mat3x3<f32>(
        vec3<f32>(1.6605, -0.1246, -0.0182),
        vec3<f32>(-0.5876, 1.1329, -0.1006),
        vec3<f32>(-0.0728, -0.0083, 1.1187)
    );
    return m * color;
}

fn aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    const a = 2.51;
    const b = 0.03;
    const c = 2.43;
    const d = 0.59;
    const e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}
fn adjust_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(vec3<f32>(luma), color, saturation);
}
@fragment
fn fs_main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {

    let y = textureSample(t_y, s_sampler, tex_coords).r;
    let u = textureSample(t_u, s_sampler, tex_coords).r;
    let v = textureSample(t_v, s_sampler, tex_coords).r;

    let yuv_input = vec3<f32>(y, u, v);


    let yuv_adjusted = yuv_input + cs_params.yuv_offset.xyz;


    if hdr_flag.flag.r==1{
    let rgb_hdr_pq = (cs_params.yuv2rgb_matrix * vec4<f32>(yuv_adjusted, 1.0)).rgb;

        let rgb_linear_abs = pq_to_linear(rgb_hdr_pq) * 10000.0;

        let rgb_scene = rgb_linear_abs / 203.0;
        let rgb_linear_bt709 = bt2020_to_bt709(rgb_scene);


        let rgb_tonemapped = aces_tonemap(rgb_linear_bt709);
        let rgb_safe_for_gamma = max(rgb_tonemapped, vec3<f32>(0.0));

        let rgb_final = pow(rgb_safe_for_gamma, vec3<f32>(1.0 / 2.2));

        let final_color = adjust_saturation(rgb_final, 0.85);
        return vec4<f32>(final_color, 1.0);
        }else{
        let final_color = (cs_params.yuv2rgb_matrix * vec4<f32>(yuv_adjusted, 1.0)).rgb;
        return vec4<f32>(final_color, 1.0);
        }
}
