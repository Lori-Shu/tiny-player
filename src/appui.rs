use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
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
    Align2, AtomExt, Button, Color32, ColorImage, CornerRadius, Id, Image, ImageData, ImageSource,
    Layout, Pos2, Rect, RichText, Stroke, TextureHandle, TextureId, TextureOptions, Ui, Vec2,
    include_image,
};

use egui_file::FileDialog;
use ffmpeg_the_third::{format::stream::Disposition, media::Type};
use flume::{Receiver, Sender, bounded};
use image::{DynamicImage, EncodableLayout, RgbaImage};

use keepawake::KeepAwake;
use rodio::Player;
use time::format_description::{self, OwnedFormatItem};
use tokio::{
    runtime::{Handle, Runtime},
    sync::{Notify, RwLock},
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult,
    controlbar_ui::ControlBarUI,
    decode::{MainStream, TinyDecoder, TinyDecoderCreationArgs},
    gpu_post_process::ColorSpaceConverter,
    internet_resource_ui::InternetResourceUI,
    playlist_ui::PlayListUI,
    present_data_manage::{DataManageContext, PresentDataManager},
    whispercpp_transcriber::{Transcriber, TranscriberArgs, UsedModel},
};

const VIDEO_FILE_IMG: ImageSource = include_image!("../resources/file-play.png");
pub const VOLUME_IMG: ImageSource = include_image!("../resources/volume-2.png");
const PLAY_IMG: ImageSource = include_image!("../resources/play.png");
const PAUSE_IMG: ImageSource = include_image!("../resources/pause.png");
pub const FULLSCREEN_IMG: ImageSource = include_image!("../resources/fullscreen.png");
const DEFAULT_BG_IMG: ImageSource = include_image!("../resources/background_2.png");
const PLAY_LIST_IMG: ImageSource = include_image!("../resources/list-video.png");
pub const SUBTITLE_IMG: ImageSource = include_image!("../resources/captions.png");
const TV_IMG: ImageSource = include_image!("../resources/tv.png");
pub const MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
const EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
pub static THEME_COLOR: LazyLock<Color32> = LazyLock::new(|| {
    let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
    orange_color[3] = 200;
    Color32::from_rgba_unmultiplied(
        orange_color[0],
        orange_color[1],
        orange_color[2],
        orange_color[3],
    )
});

/// the main struct stores all the vars which are related to ui
struct UIFlags {
    pause_flag: Arc<AtomicBool>,
    tip_window_flag: Arc<AtomicBool>,
    playlist_window_flag: Arc<AtomicBool>,
    visible_flag: Arc<AtomicBool>,
    media_source_flag: Arc<AtomicBool>,
    internet_list_window_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
}

pub struct AppUI {
    #[allow(unused)]
    async_runtime: Runtime,
    video_texture_id: Arc<RwLock<TextureId>>,
    garbage_video_texture_receiver: Receiver<TextureId>,
    tiny_decoder: Arc<RwLock<crate::decode::TinyDecoder>>,
    audio_player: Arc<crate::audio_play::AudioPlayer>,
    current_main_stream_timestamp: Arc<AtomicI64>,
    ui_flags: UIFlags,
    play_time: time::Time,
    tip_window_msg: Arc<RwLock<String>>,
    open_file_dialog: FileDialog,
    subtitle_text_receiver: Receiver<String>,
    subtitle_str: String,
    visible_num: Arc<AtomicU32>,
    wgpu_render_state: Arc<RenderState>,
    end_ts: Arc<AtomicI64>,
    internet_resource_ui: InternetResourceUI,
    change_input_context: ResetInputContext,
    playlist_ui: PlayListUI,
    time_formatter: OwnedFormatItem,
    keep_awake: Option<KeepAwake>,
    controlbar_ui: ControlBarUI,
    last_fps_update_instant: Instant,
    fps_text_str: String,
    play_tasks_notify: Arc<Notify>,
    transcribe_task_notify: Arc<Notify>,
}
impl eframe::App for AppUI {
    /// this function will automaticly be called every ui redraw
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical(|ui| {
                /*
                down part is update data part with no ui painting

                 */
                if self.manage_keepawake().is_err() {
                    warn!("manage keepawake err!");
                }
                if self
                    .ui_flags
                    .media_source_flag
                    .load(std::sync::atomic::Ordering::Acquire)
                    && !self
                        .ui_flags
                        .pause_flag
                        .load(std::sync::atomic::Ordering::Relaxed)
                    && self.is_play_end()
                {
                    self.ui_flags
                        .pause_flag
                        .store(true, std::sync::atomic::Ordering::Release);
                }

                self.clear_garbage_texture();
                /*
                down part is ui painting and control

                 */
                self.visiable_anime(ui);
                self.ui_flags
                    .visible_flag
                    .store(false, std::sync::atomic::Ordering::Release);
                self.paint_video_image(ui);
                self.paint_frame_info_text(ui);

                ui.horizontal(|ui| {
                    self.paint_tip_window(ui.ctx());
                    self.paint_file_btn(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        self.paint_playlist_button(ui);
                    });
                });

                self.paint_playpause_btn(ui);

                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    self.update_time();
                    self.update_time_text();
                    self.controlbar_ui.paint_controlbar(ui);
                    self.paint_subtitle(ui);
                });

                self.detect_file_drag(ui);
            });
        });
    }
}
impl AppUI {
    pub fn replace_fonts(&self, ctx: &egui::Context) {
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
    pub fn new(cc: &CreationContext) -> PlayerResult<Self> {
        let play_time = time::Time::from_hms(0, 0, 0)?;

        let async_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let rt = async_runtime.handle().clone();
        let f_dialog = egui_file::FileDialog::open_file();
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
        let main_color_image = Arc::new(RwLock::new(color_image));
        let wgpu_render_state = Arc::new(
            cc.wgpu_render_state
                .as_ref()
                .context("get render state err")?
                .clone(),
        );
        let media_source_flag = Arc::new(AtomicBool::new(false));
        let end_ts = Arc::new(AtomicI64::new(0));
        let hardware_config_flag = Arc::new(AtomicBool::new(false));
        let colorspace_converter = Arc::new(RwLock::new(ColorSpaceConverter::new(
            wgpu_render_state.clone(),
            cc.egui_ctx.clone(),
            hardware_config_flag.clone(),
        )?));
        let audio_frame_cache_queue = flume::bounded(32);
        let video_frame_cache_queue = flume::bounded(32);
        let audio_decode_thread_notify = Arc::new(Notify::new());
        let video_decode_thread_notify = Arc::new(Notify::new());
        let current_main_stream_timestamp = Arc::new(AtomicI64::new(0));
        let current_video_timestamp = Arc::new(AtomicI64::new(0));
        let tiny_decoder_creation_args = TinyDecoderCreationArgs::builder()
            .runtime_handle(rt.clone())
            .media_source_flag(media_source_flag.clone())
            .end_timestamp(end_ts.clone())
            .hardware_config_flag(hardware_config_flag.clone())
            .color_space_converter(colorspace_converter.clone())
            .audio_frame_cache_queue(audio_frame_cache_queue.clone())
            .video_frame_cache_queue(video_frame_cache_queue.clone())
            .audio_decode_thread_notify(audio_decode_thread_notify.clone())
            .video_decode_thread_notify(video_decode_thread_notify.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .build();
        let tiny_decoder = crate::decode::TinyDecoder::new(tiny_decoder_creation_args)?;
        let tiny_decoder = Arc::new(RwLock::new(tiny_decoder));
        let used_model = Arc::new(RwLock::new(UsedModel::None));
        let subtitle_channel = flume::bounded(10);
        let audio_player = Arc::new(crate::audio_play::AudioPlayer::new()?);

        let pause_flag = Arc::new(AtomicBool::new(false));

        let (video_texture_id, video_texture) =
            Self::alloc_texture(main_color_image.clone(), wgpu_render_state.clone());
        let present_data_task_cancellation_token = Arc::new(CancellationToken::new());
        let play_tasks_notify = Arc::new(Notify::new());
        let transcribe_task_notify = Arc::new(Notify::new());
        let transcriber_args = TranscriberArgs::builder()
            .async_runtime(rt.clone())
            .subtitle_sender(subtitle_channel.0)
            .pause_flag(pause_flag.clone())
            .used_model(used_model.clone())
            .transcribe_task_notify(transcribe_task_notify.clone())
            .build();
        let transcriber = Transcriber::new(transcriber_args)?;
        let data_manage_context = DataManageContext::builder()
            .tiny_decoder(tiny_decoder.clone())
            .used_model(used_model.clone())
            .video_texture(video_texture.clone())
            .audio_sink(audio_player.sink())
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .runtime_handle(rt.clone())
            .pause_flag(pause_flag.clone())
            .color_space_converter(colorspace_converter.clone())
            .audio_frame_receiver(audio_frame_cache_queue.1.clone())
            .video_frame_receiver(video_frame_cache_queue.1.clone())
            .audio_decode_thread_notify(audio_decode_thread_notify.clone())
            .video_decode_thread_notify(video_decode_thread_notify.clone())
            .cancellation_token(present_data_task_cancellation_token)
            .play_tasks_notify(play_tasks_notify.clone())
            .transcriber(Arc::new(RwLock::new(transcriber)))
            .build();
        let present_data_manager = PresentDataManager::new(data_manage_context);
        let present_data_manager = Arc::new(RwLock::new(present_data_manager));
        let bg_dyn_img = Arc::new(dyn_img);
        let garbage_video_texture_queue = bounded(8);
        let live_mode = Arc::new(AtomicBool::new(false));
        let tip_window_flag = Arc::new(AtomicBool::new(false));
        let tip_window_msg = Arc::new(RwLock::new("empty msg".to_string()));
        let change_input_context = ResetInputContext::builder()
            .audio_player(audio_player.sink())
            .bg_dyn_img(bg_dyn_img.clone())
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .garbage_texture_sender(garbage_video_texture_queue.0.clone())
            .main_color_image(main_color_image.clone())
            .path(PathBuf::new())
            .pause_flag(pause_flag.clone())
            .render_state(wgpu_render_state.clone())
            .runtime_handle(rt.clone())
            .tiny_decoder(tiny_decoder.clone())
            .video_texture(video_texture.clone())
            .video_texture_id(video_texture_id.clone())
            .live_mode(live_mode.clone())
            .present_data_manager(present_data_manager.clone())
            .tip_window_flag(tip_window_flag.clone())
            .tip_window_msg(tip_window_msg.clone())
            .build();
        let internet_list_window_flag = Arc::new(AtomicBool::new(false));
        let internet_resource_ui = InternetResourceUI::new(
            change_input_context.clone(),
            internet_list_window_flag.clone(),
        );
        let playlist_window_flag = Arc::new(AtomicBool::new(false));
        let playlist_ui = PlayListUI::new(
            change_input_context.clone(),
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
        let controlbar_ui = ControlBarUI::builder()
            .current_main_stream_timestamp(current_main_stream_timestamp.clone())
            .media_source_flag(media_source_flag.clone())
            .visible_flag(visible_flag.clone())
            .live_mode(live_mode.clone())
            .end_ts(end_ts.clone())
            .audio_player(audio_player.clone())
            .tiny_decoder(tiny_decoder.clone())
            .async_rt(rt.clone())
            .visible_num(visible_num.clone())
            .time_text(time_text)
            .audio_volume(audio_volume)
            .fullscreen_flag(fullscreen_flag)
            .show_volume_slider_flag(show_volume_slider_flag)
            .show_subtitle_options_flag(show_subtitle_options_flag)
            .used_model(used_model.clone())
            .transcribe_task_notify(transcribe_task_notify.clone())
            .build();
        let last_fps_update_instant = Instant::now();
        let fps_text_str = String::new();
        Ok(Self {
            async_runtime,
            garbage_video_texture_receiver: garbage_video_texture_queue.1,
            subtitle_text_receiver: subtitle_channel.1,
            subtitle_str: String::new(),
            video_texture_id,
            tiny_decoder,
            audio_player,
            current_main_stream_timestamp,
            play_time,
            ui_flags: UIFlags {
                pause_flag,
                tip_window_flag,
                playlist_window_flag,
                visible_flag,
                media_source_flag,
                internet_list_window_flag,
                live_mode,
            },
            tip_window_msg,
            open_file_dialog: f_dialog,
            visible_num,
            wgpu_render_state,
            end_ts,
            internet_resource_ui,
            change_input_context,
            playlist_ui,
            time_formatter,
            keep_awake,
            controlbar_ui,
            last_fps_update_instant,
            fps_text_str,
            play_tasks_notify,
            transcribe_task_notify,
        })
    }
    fn paint_video_image(&mut self, ui: &mut Ui) {
        /*
        show image that contains the video texture
         */
        let layer_painter = ui.ctx().layer_painter(ui.layer_id());
        if let Ok(texture_id) = self.video_texture_id.try_read() {
            layer_painter.image(
                *texture_id,
                Rect::from_min_max(
                    Pos2::new(0.0, 0.0),
                    Pos2::new(
                        ui.ctx().content_rect().width(),
                        ui.ctx().content_rect().height(),
                    ),
                ),
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    fn update_time(&mut self) {
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read()
            && self
                .ui_flags
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
            && !self
                .ui_flags
                .live_mode
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            let play_ts = self
                .current_main_stream_timestamp
                .load(std::sync::atomic::Ordering::Relaxed);
            let sec_num = {
                if let MainStream::Audio = tiny_decoder.main_stream.clone() {
                    let audio_time_base = tiny_decoder.audio_time_base;
                    play_ts * audio_time_base.numerator() as i64
                        / audio_time_base.denominator() as i64
                } else {
                    let v_time_base = tiny_decoder.video_time_base;

                    play_ts * v_time_base.numerator() as i64 / v_time_base.denominator() as i64
                }
            };
            let sec = (sec_num % 60) as u8;
            let min_num = sec_num / 60;
            let min = (min_num % 60) as u8;
            let hour_num = min_num / 60;
            let hour = hour_num as u8;
            if let Ok(cur_time) = time::Time::from_hms(hour, min, sec) {
                if cur_time != self.play_time {
                    self.play_time = cur_time;
                }
            } else {
                warn!("update time err!");
            }
        }
    }
    fn update_time_text(&mut self) {
        if let Ok(mut now_str) = self.play_time.format(&self.time_formatter)
            && let Ok(tiny_decoder) = self.tiny_decoder.try_read()
        {
            now_str.push('|');
            now_str.push_str(&tiny_decoder.end_time_formatted_string);
            self.controlbar_ui.time_text = now_str;
        }
    }
    fn alloc_texture(
        main_color_image: Arc<RwLock<ColorImage>>,
        render_state: Arc<RenderState>,
    ) -> (Arc<RwLock<TextureId>>, Arc<RwLock<Texture>>) {
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
        (
            Arc::new(RwLock::new(texture_id)),
            Arc::new(RwLock::new(video_texture)),
        )
    }
    fn free_texture(&self) {
        let texture_id = self.video_texture_id.blocking_read();
        self.wgpu_render_state
            .renderer
            .write()
            .free_texture(&texture_id);
    }
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
    fn clear_garbage_texture(&self) {
        if let Ok(garbage_texture) = self.garbage_video_texture_receiver.try_recv() {
            self.wgpu_render_state
                .renderer
                .write()
                .free_texture(&garbage_texture);
        }
    }

    fn paint_file_btn(&mut self, ui: &mut Ui) {
        let visible_num =
            f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
        let btn_rect = Vec2::new(
            ui.ctx().content_rect().width() / 20.0,
            ui.ctx().content_rect().width() / 20.0,
        );
        let file_image_button = egui::Button::new(
            Image::from(VIDEO_FILE_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(btn_rect),
        )
        .fill(egui::Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            (10.0 * visible_num) as u8,
        ))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let file_img_btn_response = ui.add(file_image_button);

        if file_img_btn_response.hovered() {
            self.ui_flags
                .visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if file_img_btn_response.clicked() {
            self.open_file_dialog.open();
        }
        let mut file_path = None;

        self.open_file_dialog.show(ui.ctx());
        if self.open_file_dialog.selected()
            && let Some(p) = self.open_file_dialog.path()
        {
            warn!("path selected{:#?}", p);
            file_path = Some(p.to_path_buf())
        }

        if let Some(p) = file_path {
            let mut ctx = self.change_input_context.clone();
            ctx.path = p.clone();
            Self::reset_media_input(ctx);
            if let Some(p_str) = p.to_str() {
                self.ui_flags
                    .live_mode
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                warn!("accept file path{}", p_str);
            }
        }
    }

    fn paint_playpause_btn(&mut self, ui: &mut Ui) {
        if self
            .ui_flags
            .media_source_flag
            .load(std::sync::atomic::Ordering::Acquire)
        {
            egui::Area::new(Id::new("playpause button area"))
                .fixed_pos(ui.content_rect().center())
                .pivot(Align2::CENTER_CENTER)
                .show(ui.ctx(), |ui| {
                    let visible_num =
                        f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    let play_or_pause_image_source = if self
                        .ui_flags
                        .pause_flag
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        PLAY_IMG
                    } else {
                        PAUSE_IMG
                    };
                    let btn_rect = Vec2::new(
                        ui.ctx().content_rect().width() / 10.0,
                        ui.ctx().content_rect().width() / 10.0,
                    );
                    let btn_img = Image::from(play_or_pause_image_source)
                        .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                        .atom_size(btn_rect);
                    let play_or_pause_btn = egui::Button::new(btn_img)
                        .fill(egui::Color32::from_rgba_unmultiplied(
                            0,
                            0,
                            0,
                            (10.0 * visible_num) as u8,
                        ))
                        .stroke(Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * visible_num) as u8),
                        ))
                        .corner_radius(CornerRadius::from(30));

                    let btn_response = ui.add(play_or_pause_btn);
                    if btn_response.hovered() {
                        self.ui_flags
                            .visible_flag
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                    if btn_response.clicked()
                        || ui.ctx().input(|s| s.key_released(egui::Key::Space))
                    {
                        let pause_flag = &self.ui_flags.pause_flag;
                        let previous_v = pause_flag.load(std::sync::atomic::Ordering::Relaxed);
                        pause_flag.store(!previous_v, std::sync::atomic::Ordering::Release);
                        let audio_player = &self.audio_player;
                        if pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            audio_player.pause();
                        } else {
                            audio_player.play();
                            self.play_tasks_notify.notify_waiters();
                            self.transcribe_task_notify.notify_one();
                        }
                    }
                });
        }
    }

    fn paint_subtitle(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self
                .ui_flags
                .media_source_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    if let Ok(generated_str) = self.subtitle_text_receiver.try_recv() {
                        self.subtitle_str = generated_str;
                    }
                    let visible_num=f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    let subtitle_text_button = egui::Button::new(
                        RichText::new(&self.subtitle_str)
                            .size(30.0)
                            .color(*THEME_COLOR)
                            .atom_size(Vec2::new(ui.content_rect().width(), 30.0)),
                    )
                    .fill(egui::Color32::from_white_alpha(
                        (10.0 * visible_num) as u8,
                    ))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_black_alpha((10.0 * visible_num) as u8),
                    ))
                    .corner_radius(CornerRadius::from(30));
                    ui.add(subtitle_text_button);
                });
            }
        });
    }

    fn paint_frame_info_text(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
            orange_color[3] = 100;
            let now_ins = Instant::now();
            if let Some(dur) = now_ins.checked_duration_since(self.last_fps_update_instant)
                && dur.as_secs() > 0
            {
                let fps = ui.input(|input| 1.0 / input.stable_dt.min(0.1));
                self.fps_text_str = format!("fps:{}", fps as i32);
                self.last_fps_update_instant = now_ins;
            }
            let rich_text = egui::RichText::new(&self.fps_text_str)
                .color(Color32::from_rgba_unmultiplied(
                    orange_color[0],
                    orange_color[1],
                    orange_color[2],
                    orange_color[3],
                ))
                .size(30.0);
            let fps_button = egui::Button::new(rich_text).frame(false);
            ui.add(fps_button);

            let mut date_time_str = "date-time：".to_string();
            if let Ok(formatter) =
                time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
                && let Ok(local_date_time) = time::OffsetDateTime::now_local()
                && let Ok(formatted_date_time_str) = local_date_time.format(&formatter)
            {
                date_time_str.push_str(formatted_date_time_str.as_str());
            }
            let rich_text = egui::RichText::new(date_time_str)
                .color(Color32::from_rgba_unmultiplied(
                    orange_color[0],
                    orange_color[1],
                    orange_color[2],
                    orange_color[3],
                ))
                .size(30.0);
            let date_time_button = egui::Button::new(rich_text).frame(false);

            ui.add(date_time_button);
        });
    }
    fn is_play_end(&self) -> bool {
        if !self
            .ui_flags
            .live_mode
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let pts = self
                .current_main_stream_timestamp
                .load(std::sync::atomic::Ordering::Relaxed);
            let end_ts = self.end_ts.load(std::sync::atomic::Ordering::Relaxed);
            if pts >= end_ts {
                warn!("play end! end_ts:{end_ts},current_ts:{pts} ");
                return true;
            }
        }
        false
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
        tiny_decoder: &TinyDecoder,
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        let cover_pic_data = tiny_decoder.cover_pic_data.clone();
        let cover_data = cover_pic_data.read().await;
        if let Some(data_vec) = &*cover_data
            && let Ok(img) = image::load_from_memory(data_vec)
        {
            let video_frame_rect = tiny_decoder.video_frame_rect;
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
    pub fn reset_media_input(context: ResetInputContext) {
        info!("in change format input");
        context.runtime_handle.spawn(async move {
            context
                .pause_flag
                .store(true, std::sync::atomic::Ordering::Release);
            context
                .current_main_stream_timestamp
                .store(0, std::sync::atomic::Ordering::Release);
            context
                .current_video_timestamp
                .store(0, std::sync::atomic::Ordering::Release);
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
                        .store(true, std::sync::atomic::Ordering::Release);
                    return;
                }

                let mut tiny_decoder = context.tiny_decoder.write().await;

                if let Err(e) = tiny_decoder.reset_input(&context.path).await {
                    let reset_input_err_msg = format!("reset_input error:{}", e);
                    warn!("reset_input error:{:?}", e);
                    *context.tip_window_msg.write().await = reset_input_err_msg;
                    context
                        .tip_window_flag
                        .store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                context.audio_player.clear();
                let video_rect = tiny_decoder.video_frame_rect;
                Self::reset_main_colorimg_to_bg(
                    context.bg_dyn_img,
                    &video_rect,
                    context.main_color_image.clone(),
                )
                .await;
                Self::reset_main_colorimg_to_cover(&tiny_decoder, context.main_color_image.clone())
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
                        .store(true, std::sync::atomic::Ordering::Release);
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
            let mut ctx = self.change_input_context.clone();
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
    fn paint_playlist_button(&mut self, ui: &mut Ui) {
        let visible_num =
            f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
        let open_btn = Button::new(
            Image::from(PLAY_LIST_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(Vec2::new(50.0, 50.0)),
        )
        .fill(egui::Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            (10.0 * visible_num) as u8,
        ))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.ui_flags
                .visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if btn_response.clicked() {
            self.ui_flags
                .playlist_window_flag
                .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self
            .ui_flags
            .playlist_window_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.playlist_ui.show(ui);
        }
        let open_btn = Button::new(
            Image::from(TV_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(Vec2::new(50.0, 50.0)),
        )
        .fill(egui::Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            (10.0 * visible_num) as u8,
        ))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.ui_flags
                .visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if btn_response.clicked() {
            self.ui_flags
                .internet_list_window_flag
                .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self
            .ui_flags
            .internet_list_window_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.internet_resource_ui.show(ui);
        }
    }
    pub async fn read_video_folder(
        ctx: egui::Context,
        path: PathBuf,
        video_des: Arc<RwLock<Vec<VideoDes>>>,
    ) {
        let mut video_targets = video_des.write().await;
        if let Ok(ite) = path.read_dir() {
            for entry in ite {
                if let Ok(en) = entry {
                    if let Ok(t) = en.file_type()
                        && let Some(file_name) = en.file_name().to_str()
                        && t.is_file()
                        && let Ok(cover) = Self::load_file_cover(&en.path()).await
                    {
                        let texture_handle =
                            Self::load_cover_texture(&ctx, &cover, file_name).await;
                        video_targets.push(VideoDes {
                            name: file_name.to_string(),
                            path: en.path(),
                            texture_handle,
                        });
                    }
                } else {
                    warn!("read dir element err");
                }
            }
        }
    }
    async fn load_file_cover(file_path: &Path) -> PlayerResult<RgbaImage> {
        if let Ok(input) = &mut ffmpeg_the_third::format::input(file_path) {
            if input.duration() < 5_000_000 {
                return Err(anyhow::Error::msg("not a valid media file"));
            }
            let mut cover_idx = None;

            for stream in input.streams() {
                if let Type::Video = stream.parameters().medium()
                    && let Disposition::ATTACHED_PIC = stream.disposition()
                {
                    cover_idx = Some(stream.index());
                    break;
                }
            }
            if let Some(idx) = cover_idx {
                for packet in input.packets() {
                    if let Ok((stream, p)) = &packet
                        && stream.index() == idx
                        && let Some(cover_data) = p.data()
                        && let Ok(dyn_img) = image::load_from_memory(cover_data)
                    {
                        return Ok(dyn_img.to_rgba8());
                    }
                }
            }
        } else {
            return Err(anyhow::Error::msg("not a valid media file"));
        }
        if let ImageSource::Bytes { bytes, .. } = PLAY_IMG
            && let Ok(dyn_img) = image::load_from_memory(bytes.as_bytes())
        {
            Ok(dyn_img.to_rgba8())
        } else {
            Err(anyhow::Error::msg("load PLAY_IMG failed"))
        }
    }
    async fn load_cover_texture(
        ctx: &egui::Context,
        cover: &RgbaImage,
        name: &str,
    ) -> TextureHandle {
        let color_image = ColorImage::from_rgba_unmultiplied(
            [cover.width() as usize, cover.height() as usize],
            cover.as_bytes(),
        );
        ctx.load_texture(
            name,
            ImageData::Color(Arc::new(color_image)),
            TextureOptions::LINEAR,
        )
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
                        .store(false, std::sync::atomic::Ordering::Release);
                }
            });
        }
    }
    fn visiable_anime(&mut self, ui: &mut Ui) {
        if !self
            .ui_flags
            .pause_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let visible_id = ui.make_persistent_id("visiable_num");
            let visible_num = ui.ctx().animate_bool_with_time(
                visible_id,
                self.ui_flags
                    .visible_flag
                    .load(std::sync::atomic::Ordering::Relaxed),
                2.0,
            );
            self.visible_num
                .store(visible_num.to_bits(), std::sync::atomic::Ordering::Release);
        } else {
            self.visible_num
                .store(1.0_f32.to_bits(), std::sync::atomic::Ordering::Release);
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
}
impl Drop for AppUI {
    fn drop(&mut self) {
        self.free_texture();
    }
}
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
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    audio_player: Arc<Player>,
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
}
