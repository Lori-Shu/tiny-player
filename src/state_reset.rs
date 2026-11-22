use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64},
    },
};

use crossfire::{MAsyncTx, mpsc::Array};
use eframe::{
    egui_wgpu::RenderState,
    wgpu::{
        Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
        TextureDimension, TextureFormat, TextureUsages, TextureView,
        wgt::{TextureDescriptor, TextureViewDescriptor},
    },
};
use egui::TextureId;
use image::{DynamicImage, EncodableLayout, RgbaImage};
use media_engine::MediaEngine;
use tokio::{runtime::Handle, sync::RwLock};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult, post_process::Transcoder, presentation::MediaDataManager,
    tip_message_ui::TipMessageType,
};

#[derive(TypedBuilder)]
pub struct StateResetter {
    pause_flag: Arc<AtomicBool>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,
    media_engine: Arc<MediaEngine>,
    bg_dyn_img: Arc<DynamicImage>,
    video_texture_id: Arc<RwLock<TextureId>>,
    render_state: Arc<RenderState>,
    garbage_texture_sender: MAsyncTx<Array<TextureId>>,
    video_texture: Arc<RwLock<Texture>>,
    runtime_handle: Handle,
    media_data_manager: Arc<RwLock<MediaDataManager>>,
    tip_modal_flag: Arc<AtomicBool>,
    tip_modal_msg: Arc<RwLock<TipMessageType>>,
    transcoder: Arc<RwLock<Transcoder>>,
}
impl StateResetter {
    /// `reset_media_input` is called when user decides to play another
    /// media. It resets the states
    /// of the decoder and the presentation manager.
    pub fn reset_media_input(self: &Arc<Self>, path: &Path) {
        info!("starting resetting input");
        let this = self.clone();
        let path = path.to_path_buf();
        self.runtime_handle.spawn(async move {
            this.pause_flag
                .store(true, std::sync::atomic::Ordering::Relaxed);
            this.current_main_stream_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            this.current_video_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            {
                let mut present_data_manager = this.media_data_manager.write().await;
                if present_data_manager.is_running
                    && let Err(e) = present_data_manager.cancel_present_tasks().await
                {
                    let stop_err_msg = format!("stop_present_tasks error:{}", e);
                    warn!("stop_present_tasks error:{:?}", e);
                    *this.tip_modal_msg.write().await = TipMessageType::Warn(stop_err_msg);
                    this.tip_modal_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                info!("starting to reset engine");
                let frame_receivers = match this.media_engine.reset_input(&path).await {
                    Err(e) => {
                        let reset_input_err_msg = format!("reset_input error:{}", e);
                        warn!("reset_input error:{:?}", e);
                        *this.tip_modal_msg.write().await =
                            TipMessageType::Warn(reset_input_err_msg);
                        this.tip_modal_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    Ok(frame_receivers) => frame_receivers,
                };

                let rgba_cover = {
                    let media_source_info = this.media_engine.media_source_info().await;
                    let mut transcoder = this.transcoder.write().await;
                    if let Some(media_source_info) = &*media_source_info {
                        if let Some(args) = &media_source_info.transcoder_args {
                            transcoder.set_params_for_space(
                                args.colorspace,
                                args.pixel_format,
                                args.transfer_characteristic,
                                [args.width, args.height],
                            );
                        }
                        this.rgba_cover_or_background(
                            &media_source_info.resolution_rect,
                            &media_source_info.cover_pic_data,
                        )
                        .await
                    } else {
                        return;
                    }
                };
                match this.update_video_texture(rgba_cover).await {
                    Ok(texture_view) => {
                        let mut transcoder = this.transcoder.write().await;
                        transcoder.update_texture_view(texture_view).await;
                    }
                    Err(e) => {
                        let update_err_msg = format!("update_video_texture error:{}", e);
                        warn!("update_video_texture error:{:?}", e);
                        *this.tip_modal_msg.write().await = TipMessageType::Warn(update_err_msg);
                        this.tip_modal_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                }
                info!("reset video texture success");
                present_data_manager
                    .spawn_present_tasks(frame_receivers)
                    .await;
                const SUCCESS_INFO: &str =
                    "media is successfully loaded! Click the play button to start playing!";
                info!(SUCCESS_INFO);
                *this.tip_modal_msg.write().await = TipMessageType::Info(SUCCESS_INFO.to_string());
                this.tip_modal_flag
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }

    async fn rgba_cover_or_background(
        &self,
        video_rect: &[u32],
        cover_pic_data: &Option<Box<[u8]>>,
    ) -> RgbaImage {
        const DEFAULT_SCALE: [u32; 2] = [1920, 1080];
        let resolution = if video_rect[0] == 0 || video_rect[1] == 0 {
            &DEFAULT_SCALE
        } else {
            video_rect
        };
        if let Some(cover_img_bytes) = &cover_pic_data
            && let Ok(img) = image::load_from_memory(cover_img_bytes)
        {
            info!("image resetting to cover");
            img.resize(
                resolution[0],
                resolution[1],
                image::imageops::FilterType::Triangle,
            )
            .to_rgba8()
        } else {
            info!("image resetting to background");
            self.bg_dyn_img
                .resize(
                    resolution[0],
                    resolution[1],
                    image::imageops::FilterType::Triangle,
                )
                .to_rgba8()
        }
    }
    /// When `reset_media_input` is called,
    /// `update_video_texture` creates a new texture and updates the cached texture.
    /// The new texture matches the new input video size.
    async fn update_video_texture(&self, rgba_img: RgbaImage) -> PlayerResult<TextureView> {
        info!("rgba_img wid{} hei{}", rgba_img.width(), rgba_img.height());
        let new_video_texture = self.render_state.device.create_texture(&TextureDescriptor {
            label: Some("Video"),
            size: Extent3d {
                width: rgba_img.width(),
                height: rgba_img.height(),
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
        let texture_view = new_video_texture.create_view(&TextureViewDescriptor {
            label: Some("Video_View"),
            format: Some(TextureFormat::Rgba8Unorm),
            aspect: TextureAspect::All,
            usage: Some(
                TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_DST,
            ),
            ..Default::default()
        });
        let new_texture_id = self.render_state.renderer.write().register_native_texture(
            &self.render_state.device,
            &texture_view,
            eframe::wgpu::FilterMode::Linear,
        );
        self.render_state.queue.write_texture(
            TexelCopyTextureInfo {
                texture: &new_video_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            rgba_img.as_bytes(),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rgba_img.width() * 4),
                rows_per_image: None,
            },
            Extent3d {
                width: rgba_img.width(),
                height: rgba_img.height(),
                depth_or_array_layers: 1,
            },
        );
        {
            let mut texture_id = self.video_texture_id.write().await;
            self.garbage_texture_sender.send(*texture_id).await?;
            *texture_id = new_texture_id;
        }

        {
            let mut video_texture = self.video_texture.write().await;
            *video_texture = new_video_texture;
        }
        Ok(texture_view)
    }
}
