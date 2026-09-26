use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
};

use eframe::{
    egui_wgpu::RenderState,
    wgpu::{
        Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
        TextureDimension, TextureFormat, TextureUsages,
        wgt::{TextureDescriptor, TextureViewDescriptor},
    },
};
use egui::{ColorImage, TextureId};
use flume::Sender;
use image::DynamicImage;
use media_engine::MediaEngine;
use tokio::{runtime::Handle, sync::RwLock};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult, audio_playback::AudioPlayer, post_process::Transcoder,
    presentation::PresentDataManager,
};

#[derive(Clone, TypedBuilder)]
pub struct StateResetter {
    pause_flag: Arc<AtomicBool>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,
    media_engine: Arc<MediaEngine>,
    audio_player: Arc<AudioPlayer>,
    main_color_image: Arc<RwLock<ColorImage>>,
    bg_dyn_img: Arc<DynamicImage>,
    video_texture_id: Arc<RwLock<TextureId>>,
    render_state: Arc<RenderState>,
    garbage_texture_sender: Sender<TextureId>,
    video_texture: Arc<RwLock<Texture>>,
    runtime_handle: Handle,
    present_data_manager: Arc<RwLock<PresentDataManager>>,
    tip_window_flag: Arc<AtomicBool>,
    tip_window_msg: Arc<RwLock<String>>,
    transcoder: Arc<RwLock<Transcoder>>,
}
impl StateResetter {
    /// `reset_media_input` is called when user decides to play another
    /// media. It resets the states
    /// of the decoder and the presentation manager.
    pub fn reset_media_input(self: &Arc<Self>, path: PathBuf) {
        info!("in change format input");
        let this = Arc::clone(self);
        self.runtime_handle.spawn(async move {
            this.pause_flag
                .store(true, std::sync::atomic::Ordering::Release);
            this.current_main_stream_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            this.current_video_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            {
                let mut present_data_manager = this.present_data_manager.write().await;
                if present_data_manager.is_running
                    && let Err(e) = present_data_manager.cancel_present_tasks().await
                {
                    let stop_err_msg = format!("stop_present_tasks error:{}", e);
                    warn!("stop_present_tasks error:{:?}", e);
                    *this.tip_window_msg.write().await = stop_err_msg;
                    this.tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                if let Err(e) = this.media_engine.reset_input(&path).await {
                    let reset_input_err_msg = format!("reset_input error:{}", e);
                    warn!("reset_input error:{:?}", e);
                    *this.tip_window_msg.write().await = reset_input_err_msg;
                    this.tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                this.audio_player.clear_source_queue();
                let media_source_info =
                    if let Ok(info) = this.media_engine.media_source_info().await {
                        info
                    } else {
                        return;
                    };
                {
                    let mut transcoder = this.transcoder.write().await;
                    if let Some(args) = &media_source_info.transcoder_args {
                        transcoder.set_params_for_space(
                            args.colorspace,
                            args.pixel_format,
                            args.transfer_characteristic,
                            [args.width, args.height],
                        );
                    }
                }
                let video_rect = media_source_info.resolution_rect;
                Self::reset_main_colorimg_to_bg(
                    this.bg_dyn_img.clone(),
                    &video_rect,
                    this.main_color_image.clone(),
                )
                .await;
                Self::reset_main_colorimg_to_cover(
                    &this.media_engine,
                    this.main_color_image.clone(),
                )
                .await;

                if let Err(e) = Self::update_video_texture(
                    this.main_color_image.clone(),
                    this.video_texture_id.clone(),
                    this.video_texture.clone(),
                    this.garbage_texture_sender.clone(),
                    this.render_state.clone(),
                )
                .await
                {
                    let update_video_texture_err_msg = format!("update_video_texture error:{}", e);
                    warn!("update_video_texture error:{:?}", e);
                    *this.tip_window_msg.write().await = update_video_texture_err_msg;
                    this.tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                info!("reset video texture success");
                present_data_manager.spawn_present_tasks();
            }
        });
    }
    async fn reset_main_colorimg_to_bg(
        bg_dyn_img: Arc<DynamicImage>,
        video_rect: &[u32; 2],
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        let bg_color_img = if video_rect[0] != 0 {
            info!(
                "before resize img width{},height{}",
                video_rect[0], video_rect[1]
            );
            let img = bg_dyn_img.resize(
                video_rect[0],
                video_rect[1],
                image::imageops::FilterType::Triangle,
            );
            ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_bytes(),
            )
        } else {
            ColorImage::from_rgba_unmultiplied(
                [bg_dyn_img.width() as usize, bg_dyn_img.height() as usize],
                bg_dyn_img.as_bytes(),
            )
        };
        let mut main_color_image = main_color_image.write().await;
        *main_color_image = bg_color_img;
    }
    async fn reset_main_colorimg_to_cover(
        media_engine: &MediaEngine,
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        info!("start reset_main_colorimg_to_cover");
        if let Ok(media_source_info) = media_engine.media_source_info().await
            && let Some(cover_img_bytes) = &media_source_info.cover_pic_data
            && let Ok(img) = image::load_from_memory(cover_img_bytes)
        {
            let video_frame_rect = media_source_info.resolution_rect;
            let rgba8_img = if video_frame_rect[0] != 0 {
                img.resize(
                    video_frame_rect[0],
                    video_frame_rect[1],
                    image::imageops::FilterType::Triangle,
                )
                .to_rgba8()
            } else {
                img.to_rgba8()
            };
            let cover_color_img = ColorImage::from_rgba_unmultiplied(
                [rgba8_img.width() as usize, rgba8_img.height() as usize],
                &rgba8_img,
            );
            info!("set cover img!");
            let mut main_color_image = main_color_image.write().await;
            *main_color_image = cover_color_img;
        }
    }
    /// When `reset_media_input` is called,
    /// `update_video_texture` creates a new texture and updates the cached texture.
    /// The new texture matches the new input video size.
    async fn update_video_texture(
        main_color_image: Arc<RwLock<ColorImage>>,
        texture_id: Arc<RwLock<TextureId>>,
        video_texture: Arc<RwLock<Texture>>,
        garbage_texture_sender: Sender<TextureId>,
        render_state: Arc<RenderState>,
    ) -> PlayerResult<()> {
        let main_color_image = main_color_image.read().await;
        info!(
            "color img wid{} hei{}",
            main_color_image.width(),
            main_color_image.height()
        );
        let new_video_texture = render_state.device.create_texture(&TextureDescriptor {
            label: Some("Video"),
            size: Extent3d {
                width: main_color_image.width() as u32,
                height: main_color_image.height() as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let new_texture_id = render_state.renderer.write().register_native_texture(
            &render_state.device,
            &new_video_texture.create_view(&TextureViewDescriptor {
                label: Some("Video_View"),
                format: Some(TextureFormat::Rgba8Unorm),
                aspect: TextureAspect::All,
                usage: Some(
                    TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_DST,
                ),
                ..Default::default()
            }),
            eframe::wgpu::FilterMode::Linear,
        );
        render_state.queue.write_texture(
            TexelCopyTextureInfo {
                texture: &new_video_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            main_color_image.as_raw(),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((main_color_image.width() * 4) as u32),
                rows_per_image: None,
            },
            Extent3d {
                width: main_color_image.width() as u32,
                height: main_color_image.height() as u32,
                depth_or_array_layers: 1,
            },
        );
        {
            let mut texture_id = texture_id.write().await;
            garbage_texture_sender.send_async(*texture_id).await?;
            *texture_id = new_texture_id;
        }

        {
            let mut video_texture = video_texture.write().await;
            *video_texture = new_video_texture;
        }
        Ok(())
    }
}
