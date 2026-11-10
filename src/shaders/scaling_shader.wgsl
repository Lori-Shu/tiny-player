struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    let x = f32(i32(in_vertex_index) / 2 % 2) * 2.0 - 1.0;
    let y = f32(i32(in_vertex_index) % 2) * 2.0 - 1.0;

    const POS = array<vec2<f32>, 6>(
            vec2<f32>(-1.0, -1.0),
            vec2<f32>(1.0, -1.0),
            vec2<f32>(-1.0, 1.0),
            vec2<f32>(-1.0, 1.0),
            vec2<f32>(1.0, -1.0),
            vec2<f32>(1.0, 1.0),
        );

    let current_pos = POS[in_vertex_index];
    out.clip_position = vec4<f32>(current_pos, 0.0, 1.0);

    out.tex_coords = vec2<f32>(current_pos.x * 0.5 + 0.5, 1.0 - (current_pos.y * 0.5 + 0.5));

    return out;
}
struct ColorSpaceUniform {
    yuv2rgb_matrix: mat4x4<f32>,
    yuv_offset: vec4<f32>,
}
struct HDRFlagUniform {
    flag: vec4<u32>,
}

@group(1) @binding(0)
var<uniform> cs_params: ColorSpaceUniform;
@group(1) @binding(1)
var<uniform> hdr_flag: HDRFlagUniform;

@group(0) @binding(0)
var t_y: texture_2d<f32>;
@group(0) @binding(1)
var t_uv: texture_2d<f32>;
@group(0) @binding(2)
var s_sampler: sampler;
fn pq_to_linear(color: vec3<f32>) -> vec3<f32> {
    const M1 = 1305.0 / 8192.0;
    const M2 = 2523.0 / 32.0;
    const C1 = 107.0 / 128.0;
    const C2 = (2413.0 / 4096.0) * 32.0;
    const C3 = (2392.0 / 4096.0) * 32.0;

    let safe_color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    let p_pow = pow(safe_color, vec3<f32>(1.0 / M2));
    let num = max(p_pow - C1, vec3<f32>(0.0));
    let den = C2 - (C3 * p_pow);
    return pow(num / den, vec3<f32>(1.0 / M1));
}

fn bt2020_to_bt709(color: vec3<f32>) -> vec3<f32> {
    const M = mat3x3<f32>(
            vec3<f32>(1.6605, -0.1246, -0.0182),
            vec3<f32>(-0.5876, 1.1329, -0.1006),
            vec3<f32>(-0.0728, -0.0083, 1.1187),
        );
    return M * color;
}
// Deprecated
// this is not the standard full aces. 
// this function is fitted approximation (commonly known as the Narkowicz ACES fit)
fn _aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    const A = 2.51;
    const B = 0.03;
    const C = 2.43;
    const D = 0.59;
    const E = 0.14;
    return clamp((x * (A * x + B)) / (x * (C * x + D) + E), vec3<f32>(0.0), vec3<f32>(1.0));
}
// Minimal Analytical AgX Tone Mapper in WGSL
// References Troy Sobotka's AgX design & polynomial curve fitting.

const AGX_MAT = mat3x3<f32>(
        vec3<f32>(0.84247906, 0.0784336, 0.07922375),
        vec3<f32>(0.04232824, 0.8784686, 0.07916613),
        vec3<f32>(0.04237565, 0.0784336, 0.87914297),
    );

const AGX_INV_MAT = mat3x3<f32>(
        vec3<f32>(1.196879, -0.0980209, -0.0990297),
        vec3<f32>(-0.0528969, 1.151903, -0.0984736),
        vec3<f32>(-0.0529716, -0.0980435, 1.151907),
    );

// 7th-order polynomial approximation of the AgX sigmoidal contrast curve
fn agx_default_contrast_approx(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    let x6 = x4 * x2;
    return -17.86 * x6 * x + 78.01 * x6 - 126.7 * x4 * x + 92.06 * x4 - 28.72 * x2 * x + 4.361 * x2
        - 0.1718 * x + 0.002857;
}

fn apply_agx(color: vec3<f32>) -> vec3<f32> {
    const MIN_EV = -12.47393;
    const MAX_EV = 0.526069;

    // 1. Inset matrix transformation (Linear sRGB -> AgX color space)
    var val = AGX_MAT * color;

    // 2. Clamp negative/zero values to prevent -INF/NaN, then encode to log2 space
    val = clamp(log2(max(val, vec3<f32>(1e-6))), vec3<f32>(MIN_EV), vec3<f32>(MAX_EV));

    // 3. Normalize log values to a [0, 1] working range
    val = (val - MIN_EV) / (MAX_EV - MIN_EV);

    // 4. Apply the sigmoidal contrast curve via polynomial evaluation
    val = agx_default_contrast_approx(val);

    // 5. Outset matrix transformation (Inverse of the inset matrix)
    val = AGX_INV_MAT * val;

    // 6. Apply display EOTF gamma to land in linear display space
    return pow(max(val, vec3<f32>(0.0)), vec3<f32>(DISPLAY_GAMMA));
}
fn adjust_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(vec3<f32>(luma), color, saturation);
}
const HDR_PEAK_NITS = 10000.0;
const SDR_REFERENCE_WHITE = 203.0;
const DISPLAY_GAMMA = 2.4;
fn pq_to_sdr(color: vec3<f32>) -> vec3<f32> {
    let rgb_hdr_pq = (cs_params.yuv2rgb_matrix * vec4<f32>(color, 1.0)).rgb;

    let rgb_linear_abs = pq_to_linear(rgb_hdr_pq) * HDR_PEAK_NITS;

    let rgb_scene = rgb_linear_abs / SDR_REFERENCE_WHITE;
    let rgb_linear_bt709 = bt2020_to_bt709(rgb_scene);

    let rgb_tonemapped = apply_agx(rgb_linear_bt709);
    let rgb_safe_for_gamma = max(rgb_tonemapped, vec3<f32>(0.0));

    let rgb_final = pow(rgb_safe_for_gamma, vec3<f32>(1.0 / DISPLAY_GAMMA));

    let final_color = adjust_saturation(rgb_final, 0.85);
    return final_color;
}

fn hlg_inverse_oetf(v: f32) -> f32 {
    const HLG_A = 0.1788;
    const HLG_B = 0.2847;
    const HLG_C = 0.5599;
    if v <= 0.5 {
        return (v * v) / 3.0;
    } else {
        return (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0;
    }
}

const REC2020_LUMA_COEFFS = vec3<f32>(0.2627, 0.6780, 0.0593);
fn hlg_to_sdr(hlg_color: vec3<f32>) -> vec3<f32> {
    var linear_rgb = vec3<f32>(
            hlg_inverse_oetf(hlg_color.r),
            hlg_inverse_oetf(hlg_color.g),
            hlg_inverse_oetf(hlg_color.b),
        );

    let luminance = dot(linear_rgb, REC2020_LUMA_COEFFS);
    if luminance > 0.0 {
        linear_rgb = linear_rgb * pow(luminance, 1.2 - 1.0);
    }

    let sdr_rgb = bt2020_to_bt709(linear_rgb);
    let rgb_tonemapped = apply_agx(sdr_rgb);
    let rgb_safe_for_gamma = max(rgb_tonemapped, vec3<f32>(0.0));

    let rgb_final = pow(rgb_safe_for_gamma, vec3<f32>(1.0 / DISPLAY_GAMMA));
    return rgb_final;
}
@fragment
fn fs_main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {
    let y = textureSample(t_y, s_sampler, tex_coords).r;
    let uv = textureSample(t_uv, s_sampler, tex_coords).rg;
    let u = uv.r;
    let v = uv.g;

    let yuv_input = vec3<f32>(y, u, v);

    let yuv_adjusted = yuv_input + cs_params.yuv_offset.xyz;
    if hdr_flag.flag.r == 1 {
        let final_color = pq_to_sdr(yuv_adjusted);
        return vec4<f32>(final_color, 1.0);
    } else if hdr_flag.flag.g == 1 {
        let final_color = hlg_to_sdr(yuv_adjusted);
        return vec4<f32>(final_color, 1.0);
    } else {
        let final_color = (cs_params.yuv2rgb_matrix * vec4<f32>(yuv_adjusted, 1.0)).rgb;
        return vec4<f32>(final_color, 1.0);
    }
}
