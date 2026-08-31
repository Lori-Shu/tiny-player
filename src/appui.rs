//! The appui module encompasses the main struct AppUI
//! which manages the user interface.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU32},
    },
    time::Instant,
};

use anyhow::Context;

use eframe::{
    CreationContext,
    egui_wgpu::RenderState,
    wgpu::{
        Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
        TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
    },
};
use egui::{
    Button, Color32, ColorImage, ImageSource, Pos2, Rect, RichText, TextureHandle, TextureId, Ui,
};

use egui_tiles::Tiles;
use flume::{Receiver, Sender, bounded};
use image::DynamicImage;

use keepawake::KeepAwake;
use time::format_description;
use tokio::{
    runtime::{Handle, Runtime},
    sync::{Notify, RwLock},
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult,
    async_clean::AsyncCleaner,
    audio_playback::AudioPlayer,
    body_ui::BodyUI,
    controlbar_ui::ControlbarUI,
    headbar_ui::{ControlPane, HeadbarUI, TreeBehavior},
    internet_resource_ui::InternetResourceUI,
    playlist_ui::PlayListUI,
    post_process::Transcoder,
    presentation::{AudioPlayContext, PresentDataManager, VideoPlayContext},
    resources::{DEFAULT_BG_IMG, EMOJI_FONT, MAPLE_FONT},
    whispercpp_transcriber::{Transcriber, TranscriberArgs, UsedModel},
};
use media_engine::MediaEngine;
/// the struct stores all bool flags corresponding to ui
struct UIFlags {
    pause_flag: Arc<AtomicBool>,
    tip_window_flag: Arc<AtomicBool>,
    visible_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
    theme_flag: bool,
}
/// the main struct stores all the vars which are related to ui.
pub struct AppUI {
    #[allow(unused)]
    async_runtime: Runtime,
    async_cleaner: Arc<RwLock<AsyncCleaner>>,
    video_texture_id: Arc<RwLock<TextureId>>,
    garbage_video_texture_receiver: Receiver<TextureId>,
    ui_flags: UIFlags,
    tip_window_msg: Arc<RwLock<String>>,
    visible_num: Arc<AtomicU32>,
    wgpu_render_state: Arc<RenderState>,
    reset_input_context: ResetInputContext,
    keep_awake: Option<KeepAwake>,
    tile_tree: egui_tiles::Tree<ControlPane>,
    tile_tree_behavior: TreeBehavior,
    fade_animation: FadeAnimation,
}
impl eframe::App for AppUI {
    /// this function will automaticly be called every ui repaint.
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        if !self.ui_flags.theme_flag {
            apply_player_visual(ui);
            replace_fonts(ui);
            apply_player_style(ui, frame);
            self.ui_flags.theme_flag = true;
            info!("set theme success");
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical(|ui| {
                // Following is the logic of ui painting
                if self.manage_keepawake().is_err() {
                    warn!("manage keepawake err!");
                }

                self.release_garbage_texture();
                /*
                down part is ui painting and control

                 */
                self.apply_visiable_animation(ui);
                self.ui_flags
                    .visible_flag
                    .store(false, std::sync::atomic::Ordering::Relaxed);

                self.paint_tip_window(ui.ctx());
                self.paint_video_image(ui);
                self.tile_tree.ui(&mut self.tile_tree_behavior, ui);

                self.detect_file_drag(ui);
                self.detect_pointer_moving(ui);
            });
        });
    }
    fn on_exit(&mut self) {
        let mut async_cleaner = self.async_cleaner.blocking_write();
        async_cleaner.start_clean();
    }
}
struct PresentationTexture {
    id: Arc<RwLock<TextureId>>,
    texture: Arc<RwLock<Texture>>,
}
struct FadeAnimation {
    start_time: Option<f64>,
    duration: f64,
}
impl FadeAnimation {
    fn new() -> Self {
        Self {
            start_time: None,
            duration: 6.0,
        }
    }
    fn trigger(&mut self, event_time_point: f64) {
        self.start_time = Some(event_time_point);
    }
    fn get_visible_num(&mut self, now: f64) -> f32 {
        match &self.start_time {
            Some(start_time) => {
                if now - (*start_time) < self.duration {
                    (1.0 - (now - (*start_time)) / self.duration) as f32
                } else {
                    self.start_time = None;
                    0.0
                }
            }
            None => 0.0,
        }
    }
}
impl AppUI {
    /// As the entry point for initializing the application,
    /// the AppUI constructor performs most of the application's initialization
    /// which instantiates the application's major components.
    pub fn new(cc: &CreationContext) -> PlayerResult<Self> {
        let play_time = time::Time::from_hms(0, 0, 0)?;

        let async_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let rt = async_runtime.handle().clone();
        let async_cleaner = Arc::new(RwLock::new(AsyncCleaner::new()));
        let (color_image, dyn_img) = {
            if let ImageSource::Bytes { bytes, .. } = DEFAULT_BG_IMG {
                let dynimg = image::load_from_memory(&bytes)?;
                Ok((
                    ColorImage::from_rgba_unmultiplied(
                        [dynimg.width() as usize, dynimg.height() as usize],
                        dynimg.as_bytes(),
                    ),
                    dynimg,
                ))
            } else {
                Err(anyhow::Error::msg("img create err"))
            }
        }?;
        let media_engine = MediaEngine::new()?;
        let main_color_image = Arc::new(RwLock::new(color_image));
        let wgpu_render_state = Arc::new(
            cc.wgpu_render_state
                .as_ref()
                .context("get render state err")?
                .clone(),
        );
        let transcoder = Arc::new(RwLock::new(Transcoder::new(
            wgpu_render_state.clone(),
            cc.egui_ctx.clone(),
            media_engine.realtime_status.hardware_config_flag.clone(),
        )?));
        let used_model = Arc::new(RwLock::new(UsedModel::None));
        let subtitle_channel = flume::bounded(10);
        let audio_player = Arc::new(crate::audio_playback::AudioPlayer::new()?);

        let pause_flag = Arc::new(AtomicBool::new(false));
        let live_mode = Arc::new(AtomicBool::new(false));
        let PresentationTexture { id, texture } =
            Self::alloc_texture(main_color_image.clone(), wgpu_render_state.clone());
        let presentation_cancellation_token = Arc::new(CancellationToken::new());
        let play_tasks_notify = Arc::new(Notify::new());
        let transcribe_task_notify = Arc::new(Notify::new());
        let transcriber_args = TranscriberArgs::builder()
            .async_runtime(rt.clone())
            .subtitle_sender(subtitle_channel.0)
            .pause_flag(pause_flag.clone())
            .used_model(used_model.clone())
            .transcribe_task_notify(transcribe_task_notify.clone())
            .async_cleaner(async_cleaner.clone())
            .build();
        let transcriber = Arc::new(RwLock::new(Transcriber::new(transcriber_args)?));
        let bars_channel = flume::bounded(128);
        let current_main_stream_timestamp = Arc::new(AtomicI64::new(0));
        let current_video_timestamp = Arc::new(AtomicI64::new(0));
        let audio_play_context = AudioPlayContext::builder()
            .audio_decode_thread_notify(
                media_engine
                    .background_tasks_notifies
                    .audio_decode_thread_notify
                    .clone(),
            )
            .audio_frame_receiver(media_engine.realtime_status.audio_frame_recv.clone())
            .audio_player(audio_player.clone())
            .cancellation_token(presentation_cancellation_token.clone())
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .pause_flag(pause_flag.clone())
            .play_tasks_notify(play_tasks_notify.clone())
            .media_engine(media_engine.clone())
            .transcriber(transcriber)
            .used_model(used_model.clone())
            .video_frame_receiver(media_engine.realtime_status.video_frame_recv.clone())
            .demux_eof_flag(media_engine.realtime_status.demux_eof_flag.clone())
            .live_mode(live_mode.clone())
            .transcoder(transcoder.clone())
            .mel_bars_sender(bars_channel.0)
            .build();
        let video_play_context = VideoPlayContext::builder()
            .cancellation_token(presentation_cancellation_token.clone())
            .transcoder(transcoder.clone())
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .pause_flag(pause_flag.clone())
            .play_tasks_notify(play_tasks_notify.clone())
            .media_engine(media_engine.clone())
            .video_decode_thread_notify(
                media_engine
                    .background_tasks_notifies
                    .video_decode_thread_notify
                    .clone(),
            )
            .video_frame_receiver(media_engine.realtime_status.video_frame_recv.clone())
            .video_texture(texture.clone())
            .audio_frame_receiver(media_engine.realtime_status.audio_frame_recv.clone())
            .demux_eof_flag(media_engine.realtime_status.demux_eof_flag.clone())
            .live_mode(live_mode.clone())
            .build();
        let present_data_manager = PresentDataManager::new(
            rt.clone(),
            presentation_cancellation_token,
            audio_play_context,
            video_play_context,
        );
        let present_data_manager = Arc::new(RwLock::new(present_data_manager));
        let bg_dyn_img = Arc::new(dyn_img);
        let garbage_video_texture_queue = bounded(8);
        let tip_window_flag = Arc::new(AtomicBool::new(false));
        let tip_window_msg = Arc::new(RwLock::new("empty msg".to_string()));
        let reset_input_context = ResetInputContext::builder()
            .audio_player(audio_player.clone())
            .bg_dyn_img(bg_dyn_img.clone())
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .garbage_texture_sender(garbage_video_texture_queue.0.clone())
            .main_color_image(main_color_image.clone())
            .path(PathBuf::new())
            .pause_flag(pause_flag.clone())
            .render_state(wgpu_render_state.clone())
            .runtime_handle(rt.clone())
            .media_engine(media_engine.clone())
            .video_texture(texture.clone())
            .video_texture_id(id.clone())
            .live_mode(live_mode.clone())
            .present_data_manager(present_data_manager.clone())
            .tip_window_flag(tip_window_flag.clone())
            .tip_window_msg(tip_window_msg.clone())
            .transcoder(transcoder.clone())
            .build();
        let internet_list_window_flag = Arc::new(AtomicBool::new(false));
        let internet_resource_ui = InternetResourceUI::new(
            reset_input_context.clone(),
            internet_list_window_flag.clone(),
        );
        let playlist_window_flag = Arc::new(AtomicBool::new(false));
        let playlist_ui = PlayListUI::new(
            reset_input_context.clone(),
            live_mode.clone(),
            rt.clone(),
            playlist_window_flag.clone(),
        );
        let time_formatter = format_description::parse_owned::<2>("[hour]:[minute]:[second]")?;
        let keep_awake = None;
        let visible_flag = Arc::new(AtomicBool::new(false));
        let visible_num = Arc::new(AtomicU32::new(1));
        let time_text = String::new();
        let audio_volume = 1.0_f32;
        let fullscreen_flag = false;
        let show_volume_slider_flag = false;
        let show_subtitle_options_flag = false;
        let controlbar_ui = ControlbarUI::builder()
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .media_source_flag(media_engine.realtime_status.media_source_flag.clone())
            .live_mode(live_mode.clone())
            .audio_player(audio_player.clone())
            .media_engine(media_engine.clone())
            .async_rt(rt.clone())
            .visible_num(visible_num.clone())
            .time_text(time_text)
            .audio_volume(audio_volume)
            .fullscreen_flag(fullscreen_flag)
            .show_volume_slider_flag(show_volume_slider_flag)
            .show_subtitle_options_flag(show_subtitle_options_flag)
            .used_model(used_model.clone())
            .transcribe_task_notify(transcribe_task_notify.clone())
            .play_time(play_time)
            .time_formatter(time_formatter)
            .build();
        let last_fps_update_instant = Instant::now();
        let fps_text_str = String::new();

        let file_dialog = egui_file_dialog::FileDialog::new();
        let headbar_ui = HeadbarUI::builder()
            .fps_text_str(fps_text_str)
            .internet_list_window_flag(internet_list_window_flag)
            .internet_resource_ui(internet_resource_ui)
            .last_fps_update_instant(last_fps_update_instant)
            .live_mode(live_mode.clone())
            .open_file_dialog(file_dialog)
            .playlist_ui(playlist_ui)
            .playlist_window_flag(playlist_window_flag)
            .reset_input_context(reset_input_context.clone())
            .visible_num(visible_num.clone())
            .build();
        let theme_flag = false;
        let body_ui = BodyUI::builder()
            .audio_player(audio_player.clone())
            .media_source_flag(media_engine.realtime_status.media_source_flag.clone())
            .pause_flag(pause_flag.clone())
            .play_tasks_notify(play_tasks_notify.clone())
            .transcribe_task_notify(transcribe_task_notify.clone())
            .visible_num(visible_num.clone())
            .subtitle_text_receiver(subtitle_channel.1)
            .subtitle_str(None)
            .last_text_time(0.0)
            .mel_bars_recv(bars_channel.1)
            .bars_buffer(Vec::new())
            .build();
        let mut tiles = Tiles::default();
        let vertical_panes = vec![
            ControlPane::Headbar(Box::new(headbar_ui)),
            ControlPane::Body(Box::new(body_ui)),
            ControlPane::Controlbar(Box::new(controlbar_ui)),
        ];
        let vertical_view = vertical_panes
            .into_iter()
            .map(|p| tiles.insert_pane(p))
            .collect();
        let root = tiles.insert_vertical_tile(vertical_view);
        let tile_tree = egui_tiles::Tree::new("player_tile_tree", root, tiles);
        let tile_tree_behavior = TreeBehavior::new();
        let fade_animation = FadeAnimation::new();
        Ok(Self {
            async_runtime,
            garbage_video_texture_receiver: garbage_video_texture_queue.1,
            video_texture_id: id,
            ui_flags: UIFlags {
                pause_flag,
                tip_window_flag,
                visible_flag,
                live_mode,
                theme_flag,
            },
            tip_window_msg,
            visible_num,
            wgpu_render_state,
            reset_input_context,
            keep_awake,
            tile_tree,
            tile_tree_behavior,
            async_cleaner,
            fade_animation,
        })
    }
    /// alloc wgpu texture and register it to the egui RenderState.
    fn alloc_texture(
        main_color_image: Arc<RwLock<ColorImage>>,
        render_state: Arc<RenderState>,
    ) -> PresentationTexture {
        let main_color_image = main_color_image.blocking_read();

        let video_texture = render_state.device.create_texture(&TextureDescriptor {
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
        let texture_id = render_state.renderer.write().register_native_texture(
            &render_state.device,
            &video_texture.create_view(&TextureViewDescriptor {
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
        info!("register texture success");
        render_state.queue.write_texture(
            TexelCopyTextureInfo {
                texture: &video_texture,
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
        PresentationTexture {
            id: Arc::new(RwLock::new(texture_id)),
            texture: Arc::new(RwLock::new(video_texture)),
        }
    }
    /// free texture, using RenderState.
    fn free_texture(&self) {
        let texture_id = self.video_texture_id.blocking_read();
        self.wgpu_render_state
            .renderer
            .write()
            .free_texture(&texture_id);
    }
    /// When `reset_media_input` is called,
    /// `update_video_texture` creates a new texture and updates the cached texture.
    /// The new texture matches the new input video rectangle.
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
    /// After updating the cached texture,
    /// `update_video_texture` sends the old TextureId to
    /// the deferred deletion queue. `clear_garbage_texture` releases the texture
    /// during the next repaint.
    /// Releasing the texture is deferred until the next repaint to avoid
    /// destroying GPU resources that may still be referenced by previously
    /// submitted commands.
    fn release_garbage_texture(&self) {
        if let Ok(garbage_texture) = self.garbage_video_texture_receiver.try_recv() {
            self.wgpu_render_state
                .renderer
                .write()
                .free_texture(&garbage_texture);
        }
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
        if let Ok(media_source_info) = media_engine.media_source_info() {
            let cover_pic_data = media_source_info.cover_pic_data.clone();
            let cover_data = cover_pic_data.read().await;
            if let Some(cover_img_bytes) = &*cover_data
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
    }
    /// `reset_media_input` is called when user decides to play another
    /// media. It resets the states
    /// of the decoder and the presentation manager.
    pub fn reset_media_input(context: ResetInputContext) {
        info!("in change format input");
        context.runtime_handle.spawn(async move {
            context
                .pause_flag
                .store(true, std::sync::atomic::Ordering::Release);
            context
                .current_main_stream_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            context
                .current_video_timestamp
                .store(0, std::sync::atomic::Ordering::Relaxed);
            {
                let mut present_data_manager = context.present_data_manager.write().await;

                if present_data_manager.is_running
                    && let Err(e) = present_data_manager.cancel_present_tasks().await
                {
                    let stop_err_msg = format!("stop_present_tasks error:{}", e);
                    warn!("stop_present_tasks error:{:?}", e);
                    *context.tip_window_msg.write().await = stop_err_msg;
                    context
                        .tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                if let Err(e) = context.media_engine.reset_input(&context.path) {
                    let reset_input_err_msg = format!("reset_input error:{}", e);
                    warn!("reset_input error:{:?}", e);
                    *context.tip_window_msg.write().await = reset_input_err_msg;
                    context
                        .tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                context.audio_player.clear_source_queue();
                let media_source_info = if let Ok(info) = context.media_engine.media_source_info() {
                    info
                } else {
                    return;
                };
                {
                    let mut transcoder = context.transcoder.write().await;
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
                    context.bg_dyn_img,
                    &video_rect,
                    context.main_color_image.clone(),
                )
                .await;
                Self::reset_main_colorimg_to_cover(
                    &context.media_engine,
                    context.main_color_image.clone(),
                )
                .await;

                if let Err(e) = Self::update_video_texture(
                    context.main_color_image,
                    context.video_texture_id,
                    context.video_texture,
                    context.garbage_texture_sender,
                    context.render_state,
                )
                .await
                {
                    let update_video_texture_err_msg = format!("update_video_texture error:{}", e);
                    warn!("update_video_texture error:{:?}", e);
                    *context.tip_window_msg.write().await = update_video_texture_err_msg;
                    context
                        .tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                info!("reset video texture success");
                present_data_manager.spawn_present_tasks();
            }
        });
    }

    fn detect_file_drag(&mut self, ui: &mut Ui) {
        let mut detected = None;
        ui.input(|input| {
            let dropped_files = &input.raw.dropped_files;
            if !dropped_files.is_empty()
                && let Some(path) = &dropped_files[0].path
            {
                detected = Some(path.to_path_buf());
            }
        });
        if let Some(path_buf) = detected {
            let mut ctx = self.reset_input_context.clone();
            ctx.path = path_buf.clone();
            Self::reset_media_input(ctx);
            if let Some(p_str) = path_buf.to_str() {
                warn!("filepath{}", p_str);
            }
            self.ui_flags
                .live_mode
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// `detect_pointer_moving` monitors the pointer movement
    /// and sets the visible_flag to true when it is moving.
    fn detect_pointer_moving(&mut self, ui: &mut Ui) {
        let (is_moving, cur_time_point) =
            ui.input(|states| (states.pointer.is_moving(), states.time));
        if is_moving {
            self.fade_animation.trigger(cur_time_point);
        }
    }

    fn paint_tip_window(&mut self, ctx: &egui::Context) {
        if self
            .ui_flags
            .tip_window_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let tip_window = egui::Window::new("tip window");
            tip_window.show(ctx, |ui| {
                let tip_text = if let Ok(tip_window_msg) = self.tip_window_msg.try_read() {
                    RichText::new(&*tip_window_msg).size(20.0)
                } else {
                    RichText::new("try read err msg failed")
                };
                ui.add(Button::new(tip_text));
                if ui.button("close").clicked() {
                    self.ui_flags
                        .tip_window_flag
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    }
    fn apply_visiable_animation(&mut self, ui: &mut Ui) {
        if !self
            .ui_flags
            .pause_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let visible_num = self.fade_animation.get_visible_num(ui.time());
            self.visible_num
                .store(visible_num.to_bits(), std::sync::atomic::Ordering::Relaxed);
        } else {
            self.visible_num
                .store(1.0_f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
        }
    }
    fn manage_keepawake(&mut self) -> PlayerResult<()> {
        if !self
            .ui_flags
            .pause_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            if self.keep_awake.is_none() {
                self.keep_awake = Some(
                    keepawake::Builder::default()
                        .display(true)
                        .idle(true)
                        .app_name("tiny-player")
                        .reason("video play")
                        .create()?,
                );
            }
        } else {
            if self.keep_awake.is_some() {
                self.keep_awake.take();
            }
        }
        Ok(())
    }
    /// paint image from the video texture.
    fn paint_video_image(&mut self, ui: &mut Ui) {
        let layer_painter = ui.painter();
        if let Ok(texture_id) = self.video_texture_id.try_read() {
            layer_painter.image(
                *texture_id,
                ui.content_rect(),
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}
impl Drop for AppUI {
    fn drop(&mut self) {
        self.free_texture();
    }
}

fn apply_player_visual(ctx: &Ui) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = egui::Color32::from_rgb(15, 23, 42);

    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(30, 41, 59);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(51, 65, 85);

    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(6, 182, 212);
    visuals.selection.bg_fill = egui::Color32::from_rgb(6, 182, 212).linear_multiply(0.3);
    visuals.window_fill = Color32::from_rgb(2, 6, 23);

    visuals.panel_fill = Color32::from_rgb(15, 23, 42);
    ctx.set_visuals(visuals);
}
fn apply_player_style(ctx: &Ui, _frame: &mut eframe::Frame) {
    let mut style = (*ctx.global_style()).clone();

    style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
    style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);

    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    ctx.set_global_style(style);
}
fn replace_fonts(ctx: &Ui) {
    // Start with the default fonts (we will be adding to them rather than replacing them).
    let mut fonts = egui::FontDefinitions::default();

    // Install my own font (maybe supporting non-latin characters).
    // .ttf and .otf files supported.
    fonts.font_data.insert(
        "app_default_font".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(MAPLE_FONT)),
    );
    fonts.font_data.insert(
        "noto_emoji".to_owned(),
        Arc::new(egui::FontData::from_static(EMOJI_FONT)),
    );
    // Put my font first (highest priority) for proportional text:
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "app_default_font".to_owned());

    // Put my font as last fallback for monospace:
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "app_default_font".to_owned());

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(1, "noto_emoji".to_owned());
    // Tell egui to use these fonts:
    ctx.set_fonts(fonts);
}
#[derive(Clone)]
pub struct VideoDes {
    pub name: String,
    pub path: PathBuf,
    pub texture_handle: TextureHandle,
}
#[derive(Clone, TypedBuilder)]
pub struct ResetInputContext {
    pause_flag: Arc<AtomicBool>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    current_video_timestamp: Arc<AtomicI64>,
    media_engine: Arc<MediaEngine>,
    audio_player: Arc<AudioPlayer>,
    main_color_image: Arc<RwLock<ColorImage>>,
    bg_dyn_img: Arc<DynamicImage>,
    video_texture_id: Arc<RwLock<TextureId>>,
    render_state: Arc<RenderState>,
    pub path: PathBuf,
    garbage_texture_sender: Sender<TextureId>,
    video_texture: Arc<RwLock<Texture>>,
    pub runtime_handle: Handle,
    pub live_mode: Arc<AtomicBool>,
    present_data_manager: Arc<RwLock<PresentDataManager>>,
    tip_window_flag: Arc<AtomicBool>,
    tip_window_msg: Arc<RwLock<String>>,
    transcoder: Arc<RwLock<Transcoder>>,
}
