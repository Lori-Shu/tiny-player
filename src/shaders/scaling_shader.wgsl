
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
    yuv2rgb_matrix: mat3x3<f32>,
    yuv_offset: vec3<f32>,
    _padding: f32,
}


@group(1) @binding(0) var<uniform> cs_params: ColorSpaceUniform;

@group(0) @binding(0) var t_y: texture_2d<f32>;
@group(0) @binding(1) var t_uv: texture_2d<f32>;
@group(0) @binding(2) var s_sampler: sampler;

@fragment
fn fs_main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {
    let y = textureSample(t_y, s_sampler, tex_coords).r;
    let y_norm = (y - 0.063) * 1.164;
    let uv = textureSample(t_uv, s_sampler, tex_coords).rg;
    let u = uv.r;
    let v = uv.g;

    let yuv = vec3<f32>(y_norm, u, v) + cs_params.yuv_offset;
    let rgb = cs_params.yuv2rgb_matrix * yuv;

    return vec4<f32>(rgb, 1.0);
}
