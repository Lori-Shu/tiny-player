use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Context;
use eframe::{
    CreationContext,
    egui_wgpu::RenderState,
    wgpu::{
        AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
        BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, BlendState,
        Buffer, BufferBindingType, BufferUsages, Color, ColorTargetState, ColorWrites,
        CommandEncoderDescriptor, Extent3d, FilterMode, FragmentState, LoadOp, MipmapFilterMode,
        MultisampleState, Operations, Origin3d, PipelineCompilationOptions,
        PipelineLayoutDescriptor, PrimitiveState, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipeline, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
        ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp, TexelCopyBufferLayout,
        TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor, TextureDimension,
        TextureFormat, TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor,
        TextureViewDimension, VertexState,
        util::{BufferInitDescriptor, DeviceExt},
    },
};

use ffmpeg_the_third::{color::Space, format::Pixel, frame::Video};
use glam::{Mat3, Vec3};
use tokio::sync::RwLock;
use tracing::info;

use crate::PlayerResult;
const SCALING_SHADER: &str = include_str!("./shaders/scaling_shader.wgsl");
const FALLBACK_SCALING_SHADER: &str = include_str!("./shaders/fallback_scaling_shader.wgsl");
pub struct ColorSpaceConverter {
    bt709_uniform: ColorSpaceUniform,
    bt601_uniform: ColorSpaceUniform,
    bt2020_uniform: ColorSpaceUniform,
    uniform_buffer: Buffer,
    bind_group_layout: BindGroupLayout,
    fallback_bind_group_layout: BindGroupLayout,
    uniform_bind_group: BindGroup,
    bind_group: Option<BindGroup>,
    fallback_bind_group: Option<BindGroup>,
    texture_y: Option<Texture>,
    texture_u: Option<Texture>,
    texture_v: Option<Texture>,
    texture_uv: Option<Texture>,
    sampler: Sampler,
    render_pipeline: RenderPipeline,
    fallback_render_pipeline: RenderPipeline,
    playback_texture_view: Option<TextureView>,
}
impl ColorSpaceConverter {
    pub fn new(cc: &CreationContext) -> PlayerResult<Self> {
        let bt709_uniform = Self::get_bt709_params();
        let bt601_uniform = Self::get_bt601_params();
        let bt2020_uniform = Self::get_bt2020_params();
        let render_state = cc
            .wgpu_render_state
            .clone()
            .context("render state get error")?;
        let uniform_buffer = render_state
            .device
            .create_buffer_init(&BufferInitDescriptor {
                label: Some("Color Space Uniform Buffer"),
                contents: bytemuck::cast_slice(&[bt709_uniform]),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });
        let bind_group_layout =
            render_state
                .device
                .create_bind_group_layout(&BindGroupLayoutDescriptor {
                    entries: &[
                        BindGroupLayoutEntry {
                            binding: 0,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Texture {
                                sample_type: TextureSampleType::Float { filterable: true },
                                view_dimension: eframe::wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        BindGroupLayoutEntry {
                            binding: 1,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Texture {
                                sample_type: TextureSampleType::Float { filterable: true },
                                view_dimension: eframe::wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        BindGroupLayoutEntry {
                            binding: 2,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Sampler(SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                    label: Some("bind_group_layout"),
                });
        let fallback_bind_group_layout =
            render_state
                .device
                .create_bind_group_layout(&BindGroupLayoutDescriptor {
                    entries: &[
                        BindGroupLayoutEntry {
                            binding: 0,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Texture {
                                sample_type: TextureSampleType::Float { filterable: true },
                                view_dimension: eframe::wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        BindGroupLayoutEntry {
                            binding: 1,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Texture {
                                sample_type: TextureSampleType::Float { filterable: true },
                                view_dimension: eframe::wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        BindGroupLayoutEntry {
                            binding: 2,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Texture {
                                sample_type: TextureSampleType::Float { filterable: true },
                                view_dimension: eframe::wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        BindGroupLayoutEntry {
                            binding: 3,
                            visibility: ShaderStages::FRAGMENT,
                            ty: BindingType::Sampler(SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                    label: Some("fallback_bind_group_layout"),
                });
        let sampler = render_state.device.create_sampler(&SamplerDescriptor {
            label: Some("Video_Frame_Sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });
        let uniform_bind_group_layout =
            render_state
                .device
                .create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("ColorSpace_Uniform_Layout"),
                    entries: &[BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
        let uniform_bind_group = render_state.device.create_bind_group(&BindGroupDescriptor {
            layout: &uniform_bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("ColorSpace_Uniform_BindGroup"),
        });
        let scaling_shader = render_state
            .device
            .create_shader_module(ShaderModuleDescriptor {
                label: Some("Scaling_Shader"),
                source: ShaderSource::Wgsl(SCALING_SHADER.into()),
            });
        let fallback_scaling_shader =
            render_state
                .device
                .create_shader_module(ShaderModuleDescriptor {
                    label: Some("Fallback_Scaling_Shader"),
                    source: ShaderSource::Wgsl(FALLBACK_SCALING_SHADER.into()),
                });
        let pipeline_layout =
            render_state
                .device
                .create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("Pipeline_Layout"),
                    bind_group_layouts: &[
                        Some(&bind_group_layout),
                        Some(&uniform_bind_group_layout),
                    ],
                    immediate_size: 0,
                });
        let fallback_pipeline_layout =
            render_state
                .device
                .create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("Fallback_Pipeline_Layout"),
                    bind_group_layouts: &[
                        Some(&fallback_bind_group_layout),
                        Some(&uniform_bind_group_layout),
                    ],
                    immediate_size: 0,
                });
        let render_pipeline =
            render_state
                .device
                .create_render_pipeline(&RenderPipelineDescriptor {
                    label: Some("Video_Render_Pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: VertexState {
                        module: &scaling_shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: PipelineCompilationOptions::default(),
                    },
                    fragment: Some(FragmentState {
                        module: &scaling_shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(ColorTargetState {
                            format: TextureFormat::Rgba8Unorm,
                            blend: Some(BlendState::REPLACE),
                            write_mask: ColorWrites::ALL,
                        })],
                        compilation_options: PipelineCompilationOptions::default(),
                    }),
                    primitive: PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
        let fallback_render_pipeline =
            render_state
                .device
                .create_render_pipeline(&RenderPipelineDescriptor {
                    label: Some("Fallback_Video_Render_Pipeline"),
                    layout: Some(&fallback_pipeline_layout),
                    vertex: VertexState {
                        module: &fallback_scaling_shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: PipelineCompilationOptions::default(),
                    },
                    fragment: Some(FragmentState {
                        module: &fallback_scaling_shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(ColorTargetState {
                            format: TextureFormat::Rgba8Unorm,
                            blend: Some(BlendState::REPLACE),
                            write_mask: ColorWrites::ALL,
                        })],
                        compilation_options: PipelineCompilationOptions::default(),
                    }),
                    primitive: PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
        Ok(Self {
            bt709_uniform,
            bt601_uniform,
            bt2020_uniform,
            uniform_buffer,
            texture_y: None,
            texture_u: None,
            texture_v: None,
            texture_uv: None,
            bind_group_layout,
            fallback_bind_group_layout,
            bind_group: None,
            fallback_bind_group: None,
            sampler,
            uniform_bind_group,
            render_pipeline,
            fallback_render_pipeline,
            playback_texture_view: None,
        })
    }
    fn get_bt601_params() -> ColorSpaceUniform {
        let m = glam::Mat3::from_cols_array(&[
            1.164, 1.164, 1.164, 0.0, -0.391, 2.018, 1.596, -0.813, 0.0,
        ]);
        let o = glam::Vec3::new(-16.0 / 255.0, -128.0 / 255.0, -128.0 / 255.0);
        ColorSpaceUniform::new(m, o)
    }
    fn get_bt709_params() -> ColorSpaceUniform {
        let m = glam::Mat3::from_cols_array(&[
            1.164, 1.164, 1.164, 0.0, -0.213, 2.112, 1.793, -0.533, 0.0,
        ]);
        let o = glam::Vec3::new(-16.0 / 255.0, -128.0 / 255.0, -128.0 / 255.0);
        ColorSpaceUniform::new(m, o)
    }
    fn get_bt2020_params() -> ColorSpaceUniform {
        let m = glam::Mat3::from_cols_array(&[
            1.164, 1.164, 1.164, 0.0, -0.187, 2.142, 1.675, -0.650, 0.0,
        ]);
        let o = glam::Vec3::new(-16.0 / 255.0, -128.0 / 255.0, -128.0 / 255.0);
        ColorSpaceUniform::new(m, o)
    }
    pub fn set_params_for_space(
        &mut self,
        render_state: &RenderState,
        color_space: Space,
        pixel_format: Pixel,
        size_rect: [u32; 2],
        hardware_flag: Arc<AtomicBool>,
    ) {
        let color_space_uniform = match color_space {
            Space::BT709 => self.bt709_uniform,
            Space::SMPTE170M | Space::BT470BG => self.bt601_uniform,
            Space::BT2020NCL | Space::BT2020CL => self.bt2020_uniform,
            _ => self.bt709_uniform,
        };
        render_state.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[color_space_uniform]),
        );
        let texture_y = if pixel_format != Pixel::P010LE
            && pixel_format != Pixel::YUV420P10
            && pixel_format != Pixel::YUV420P10LE
        {
            render_state.device.create_texture(&TextureDescriptor {
                label: Some("Video_Y_Plane"),
                size: Extent3d {
                    width: size_rect[0],
                    height: size_rect[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            })
        } else {
            render_state.device.create_texture(&TextureDescriptor {
                label: Some("Video_Y_Plane"),
                size: Extent3d {
                    width: size_rect[0],
                    height: size_rect[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R16Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };

        if hardware_flag.load(std::sync::atomic::Ordering::Acquire) {
            if pixel_format != Pixel::P010LE
                && pixel_format != Pixel::YUV420P10
                && pixel_format != Pixel::YUV420P10LE
            {
                let texture_uv = render_state.device.create_texture(&TextureDescriptor {
                    label: Some("Video_NV12_UV_Plane"),
                    size: Extent3d {
                        width: size_rect[0] / 2,
                        height: size_rect[1] / 2,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rg8Unorm,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    view_formats: &[],
                });

                self.bind_group =
                    Some(render_state.device.create_bind_group(&BindGroupDescriptor {
                        layout: &self.bind_group_layout,
                        entries: &[
                            BindGroupEntry {
                                binding: 0,
                                resource: BindingResource::TextureView(&texture_y.create_view(
                                    &TextureViewDescriptor {
                                        label: Some("Video_Y_View"),
                                        format: Some(TextureFormat::R8Unorm),
                                        dimension: Some(TextureViewDimension::D2),
                                        aspect: TextureAspect::All,
                                        base_mip_level: 0,
                                        mip_level_count: None,
                                        base_array_layer: 0,
                                        array_layer_count: None,
                                        usage: Some(
                                            TextureUsages::TEXTURE_BINDING
                                                | TextureUsages::COPY_DST,
                                        ),
                                    },
                                )),
                            },
                            BindGroupEntry {
                                binding: 1,
                                resource: BindingResource::TextureView(&texture_uv.create_view(
                                    &TextureViewDescriptor {
                                        label: Some("Video_UV_View"),
                                        format: Some(TextureFormat::Rg8Unorm),
                                        aspect: TextureAspect::All,
                                        ..Default::default()
                                    },
                                )),
                            },
                            BindGroupEntry {
                                binding: 2,
                                resource: BindingResource::Sampler(&self.sampler),
                            },
                        ],
                        label: Some("video_frame_bind_group"),
                    }));

                self.texture_uv = Some(texture_uv);
            } else {
                let texture_uv = render_state.device.create_texture(&TextureDescriptor {
                    label: Some("Video_NV12_UV_Plane"),
                    size: Extent3d {
                        width: size_rect[0] / 2,
                        height: size_rect[1] / 2,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rg16Unorm,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    view_formats: &[],
                });

                self.bind_group =
                    Some(render_state.device.create_bind_group(&BindGroupDescriptor {
                        layout: &self.bind_group_layout,
                        entries: &[
                            BindGroupEntry {
                                binding: 0,
                                resource: BindingResource::TextureView(&texture_y.create_view(
                                    &TextureViewDescriptor {
                                        label: Some("Video_Y_View"),
                                        format: Some(TextureFormat::R16Unorm),
                                        dimension: Some(TextureViewDimension::D2),
                                        aspect: TextureAspect::All,
                                        base_mip_level: 0,
                                        mip_level_count: None,
                                        base_array_layer: 0,
                                        array_layer_count: None,
                                        usage: Some(
                                            TextureUsages::TEXTURE_BINDING
                                                | TextureUsages::COPY_DST,
                                        ),
                                    },
                                )),
                            },
                            BindGroupEntry {
                                binding: 1,
                                resource: BindingResource::TextureView(&texture_uv.create_view(
                                    &TextureViewDescriptor {
                                        label: Some("Video_UV_View"),
                                        format: Some(TextureFormat::Rg16Unorm),
                                        aspect: TextureAspect::All,
                                        ..Default::default()
                                    },
                                )),
                            },
                            BindGroupEntry {
                                binding: 2,
                                resource: BindingResource::Sampler(&self.sampler),
                            },
                        ],
                        label: Some("video_frame_bind_group"),
                    }));

                self.texture_uv = Some(texture_uv);
            }
        } else {
            let texture_u = render_state.device.create_texture(&TextureDescriptor {
                label: Some("Video_U_Plane"),
                size: Extent3d {
                    width: size_rect[0] / 2,
                    height: size_rect[1] / 2,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let texture_v = render_state.device.create_texture(&TextureDescriptor {
                label: Some("Video_V_Plane"),
                size: Extent3d {
                    width: size_rect[0] / 2,
                    height: size_rect[1] / 2,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R8Unorm,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.fallback_bind_group =
                Some(render_state.device.create_bind_group(&BindGroupDescriptor {
                    layout: &self.fallback_bind_group_layout,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&texture_y.create_view(
                                &TextureViewDescriptor {
                                    label: Some("Video_Y_View"),
                                    format: Some(TextureFormat::R8Unorm),
                                    dimension: Some(TextureViewDimension::D2),
                                    aspect: TextureAspect::All,
                                    base_mip_level: 0,
                                    mip_level_count: None,
                                    base_array_layer: 0,
                                    array_layer_count: None,
                                    usage: Some(
                                        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                                    ),
                                },
                            )),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::TextureView(&texture_u.create_view(
                                &TextureViewDescriptor {
                                    label: Some("Video_U_View"),
                                    format: Some(TextureFormat::R8Unorm),
                                    dimension: Some(TextureViewDimension::D2),
                                    aspect: TextureAspect::All,
                                    base_mip_level: 0,
                                    mip_level_count: None,
                                    base_array_layer: 0,
                                    array_layer_count: None,
                                    usage: Some(
                                        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                                    ),
                                },
                            )),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: BindingResource::TextureView(&texture_v.create_view(
                                &TextureViewDescriptor {
                                    label: Some("Video_V_View"),
                                    format: Some(TextureFormat::R8Unorm),
                                    dimension: Some(TextureViewDimension::D2),
                                    aspect: TextureAspect::All,
                                    base_mip_level: 0,
                                    mip_level_count: None,
                                    base_array_layer: 0,
                                    array_layer_count: None,
                                    usage: Some(
                                        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                                    ),
                                },
                            )),
                        },
                        BindGroupEntry {
                            binding: 3,
                            resource: BindingResource::Sampler(&self.sampler),
                        },
                    ],
                    label: Some("fallback_video_frame_bind_group"),
                }));
            self.texture_u = Some(texture_u);
            self.texture_v = Some(texture_v);
        }
        self.texture_y = Some(texture_y);
        self.playback_texture_view = None;
        info!("reset playback_texture_view success");
    }
    pub async fn render_video(
        &mut self,
        render_state: &RenderState,
        egui_ctx: &egui::Context,
        texture: Arc<RwLock<Texture>>,
        frame: Video,
        is_hw_acc: bool,
    ) -> PlayerResult<()> {
        let playback_texture_view = if let Some(view) = &self.playback_texture_view {
            view.clone()
        } else {
            let updated_texture = texture.read().await;
            let new_texture_view = updated_texture.create_view(&TextureViewDescriptor {
                label: Some("Video_Playback_View"),
                format: Some(TextureFormat::Rgba8Unorm),
                dimension: Some(TextureViewDimension::D2),
                aspect: TextureAspect::All,
                base_mip_level: 0,
                mip_level_count: None,
                base_array_layer: 0,
                array_layer_count: None,
                usage: Some(
                    TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_DST,
                ),
            });
            self.playback_texture_view = Some(new_texture_view.clone());
            new_texture_view
        };
        if is_hw_acc {
            if let Some(texture) = &self.texture_y {
                render_state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    frame.data(0),
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride(0) as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: frame.plane_width(0),
                        height: frame.plane_height(0),
                        depth_or_array_layers: 1,
                    },
                );
            }
            if let Some(texture) = &self.texture_uv {
                render_state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    frame.data(1),
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride(1) as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: frame.plane_width(1),
                        height: frame.plane_height(1),
                        depth_or_array_layers: 1,
                    },
                );
            }
            let mut encoder =
                render_state
                    .device
                    .create_command_encoder(&CommandEncoderDescriptor {
                        label: Some("Offscreen_Render_Encoder"),
                    });

            {
                let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Video_To_Texture_Pass"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &playback_texture_view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(Color::BLACK),
                            store: StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                render_pass.set_pipeline(&self.render_pipeline);
                render_pass.set_bind_group(0, &self.bind_group, &[]);
                render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);

                render_pass.draw(0..6, 0..1);
            }

            render_state.queue.submit(std::iter::once(encoder.finish()));
            // warn!("after write texture and render");
        } else {
            if let Some(texture) = &self.texture_y {
                render_state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    frame.data(0),
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride(0) as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: frame.plane_width(0),
                        height: frame.plane_height(0),
                        depth_or_array_layers: 1,
                    },
                );
            }
            if let Some(texture) = &self.texture_u {
                render_state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    frame.data(1),
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride(1) as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: frame.plane_width(1),
                        height: frame.plane_height(1),
                        depth_or_array_layers: 1,
                    },
                );
            }
            if let Some(texture) = &self.texture_v {
                render_state.queue.write_texture(
                    TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    frame.data(2),
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride(2) as u32),
                        rows_per_image: None,
                    },
                    Extent3d {
                        width: frame.plane_width(2),
                        height: frame.plane_height(2),
                        depth_or_array_layers: 1,
                    },
                );
            }
            let mut encoder =
                render_state
                    .device
                    .create_command_encoder(&CommandEncoderDescriptor {
                        label: Some("Offscreen_Render_Encoder"),
                    });

            {
                let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Video_To_Texture_Pass"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &playback_texture_view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(Color::BLACK),
                            store: StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                render_pass.set_pipeline(&self.fallback_render_pipeline);
                render_pass.set_bind_group(0, &self.fallback_bind_group, &[]);
                render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);

                render_pass.draw(0..6, 0..1);
            }

            render_state.queue.submit(std::iter::once(encoder.finish()));
        }
        egui_ctx.request_repaint();
        Ok(())
    }
}
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ColorSpaceUniform {
    matrix: [[f32; 4]; 3],
    offset: [f32; 3],
    _padding: f32,
}

impl ColorSpaceUniform {
    fn new(matrix: Mat3, offset: Vec3) -> Self {
        Self {
            matrix: [
                [matrix.x_axis[0], matrix.x_axis[1], matrix.x_axis[2], 0.0],
                [matrix.y_axis[0], matrix.y_axis[1], matrix.y_axis[2], 0.0],
                [matrix.z_axis[0], matrix.z_axis[1], matrix.z_axis[2], 0.0],
            ],
            offset: offset.into(),
            _padding: 0.0,
        }
    }
}
