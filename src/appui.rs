use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicI64},
    },
    time::Instant,
};

use anyhow::Context;
use derive_builder::Builder;
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
use tracing::{info, warn};

use crate::{
    PlayerResult,
    controlbar_ui::ControlBarUI,
    decode::{MainStream, TinyDecoder},
    gpu_post_process::ColorSpaceConverter,
    internet_resource_ui::InternetResourceUI,
    playlist_ui::PlayListUI,
    present_data_manage::{DataManageContextBuilder, PresentDataManager},
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
    tip_window_flag: bool,
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
    tip_window_msg: String,
    app_start_instant: Instant,
    open_file_dialog: FileDialog,
    // _subtitle: Arc<RwLock<AISubTitle>>,
    // subtitle_text: String,
    // subtitle_text_receiver: mpsc::Receiver<String>,
    // used_model: Arc<RwLock<UsedModel>>,
    visible_num: Arc<RwLock<f32>>,
    wgpu_render_state: Arc<RenderState>,
    end_ts: Arc<AtomicI64>,
    internet_resource_ui: InternetResourceUI,
    change_input_context: ChangeInputContext,
    playlist_ui: PlayListUI,
    time_formatter: OwnedFormatItem,
    keep_awake: Option<KeepAwake>,
    controlbar_ui: ControlBarUI,
}
impl eframe::App for AppUI {
    /// this function will automaticly be called every ui redraw
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical(|ui| {
                /*
                down part is update data part with no ui painting

                 */
                let now = Instant::now();
                if self.manage_keepawake().is_err() {
                    warn!("manage keepawake err!");
                }
                if self
                    .ui_flags
                    .media_source_flag
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    if !self
                        .ui_flags
                        .pause_flag
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        if self.check_play_end() {
                            self.ui_flags
                                .pause_flag
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                    }
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
                self.paint_frame_info_text(ui, &now);

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
                    // self.paint_subtitle(ui, ctx);
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
        let tiny_decoder = crate::decode::TinyDecoder::new(
            rt.clone(),
            media_source_flag.clone(),
            end_ts.clone(),
            hardware_config_flag.clone(),
            colorspace_converter.clone(),
            audio_frame_cache_queue.clone(),
            video_frame_cache_queue.clone(),
            audio_decode_thread_notify.clone(),
            video_decode_thread_notify.clone(),
        )?;
        let tiny_decoder = Arc::new(RwLock::new(tiny_decoder));
        // let used_model = Arc::new(RwLock::new(UsedModel::Empty));
        // let subtitle_channel = mpsc::channel(10);
        // let subtitle = Arc::new(RwLock::new(AISubTitle::new(subtitle_channel.0)?));
        let audio_player = Arc::new(crate::audio_play::AudioPlayer::new()?);
        let current_main_stream_timestamp = Arc::new(AtomicI64::new(0));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let current_video_timestamp = Arc::new(AtomicI64::new(0));

        let (video_texture_id, video_texture) =
            Self::alloc_texture(main_color_image.clone(), wgpu_render_state.clone());
        let data_manage_context = DataManageContextBuilder::default()
            .tiny_decoder(tiny_decoder.clone())
            // .used_model(used_model.clone())
            // .ai_subtitle(subtitle.clone())
            .video_texture(video_texture.clone())
            .audio_sink(audio_player.sink())
            .main_stream_current_timestamp(current_main_stream_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .runtime_handle(rt.clone())
            .pause_flag(pause_flag.clone())
            .color_space_converter(colorspace_converter.clone())
            .audio_frame_receiver(audio_frame_cache_queue.1.clone())
            .video_frame_receiver(video_frame_cache_queue.1.clone())
            .audio_decode_thread_notify(audio_decode_thread_notify.clone())
            .video_decode_thread_notify(video_decode_thread_notify.clone())
            .build()?;
        let mut present_data_manager = PresentDataManager::new(data_manage_context);
        present_data_manager.start_present_tasks();
        let present_data_manager = Arc::new(RwLock::new(present_data_manager));
        let bg_dyn_img = Arc::new(dyn_img);
        let garbage_video_texture_queue = bounded(8);
        let live_mode = Arc::new(AtomicBool::new(false));
        let change_input_context = ChangeInputContextBuilder::default()
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
            .build()?;
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
        let visible_num = Arc::new(RwLock::new(1.0_f32));
        let controlbar_ui = ControlBarUI::new(
            current_main_stream_timestamp.clone(),
            media_source_flag.clone(),
            visible_flag.clone(),
            live_mode.clone(),
            end_ts.clone(),
            audio_player.clone(),
            tiny_decoder.clone(),
            rt.clone(),
            pause_flag.clone(),
            visible_num.clone(),
        );
        Ok(Self {
            async_runtime,
            garbage_video_texture_receiver: garbage_video_texture_queue.1,
            // subtitle_text_receiver: subtitle_channel.1,
            video_texture_id,
            tiny_decoder,
            audio_player,
            current_main_stream_timestamp,
            play_time,
            ui_flags: UIFlags {
                pause_flag,
                tip_window_flag: false,
                playlist_window_flag,
                visible_flag,
                media_source_flag,
                internet_list_window_flag,
                live_mode,
            },
            // used_model,
            tip_window_msg: String::new(),
            app_start_instant: Instant::now(),
            open_file_dialog: f_dialog,
            // _subtitle: subtitle,
            // subtitle_text: String::new(),
            visible_num,
            wgpu_render_state,
            end_ts,
            internet_resource_ui,
            change_input_context,
            playlist_ui,
            time_formatter,
            keep_awake,
            controlbar_ui,
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
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
            if self
                .ui_flags
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
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
    }
    fn update_time_text(&mut self) {
        if let Ok(mut now_str) = self.play_time.format(&self.time_formatter) {
            if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
                now_str.push('|');
                now_str.push_str(&tiny_decoder.end_time_formatted_string);
                self.controlbar_ui.time_text = now_str;
            }
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
        if let Ok(visible_num) = self.visible_num.try_read() {
            let btn_rect = Vec2::new(
                ui.ctx().content_rect().width() / 10.0,
                ui.ctx().content_rect().width() / 10.0,
            );
            let file_image_button = egui::Button::new(
                Image::from(VIDEO_FILE_IMG)
                    .tint(Color32::from_white_alpha((255.0 * *visible_num) as u8))
                    .atom_size(btn_rect),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                0,
                0,
                0,
                (10.0 * *visible_num) as u8,
            ))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * *visible_num) as u8),
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
            if self.open_file_dialog.selected() {
                if let Some(p) = self.open_file_dialog.path() {
                    warn!("path selected{:#?}", p);
                    file_path = Some(p.to_path_buf())
                }
            }

            if let Some(p) = file_path {
                let mut ctx = self.change_input_context.clone();
                ctx.path = p.clone();
                if Self::change_format_input(ctx).is_ok() {
                    if let Some(p_str) = p.to_str() {
                        self.ui_flags
                            .live_mode
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        warn!("accept file path{}", p_str);
                    }
                } else {
                    self.tip_window_msg =
                        "please choose a valid video or audio file !!!".to_string();
                    self.ui_flags.tip_window_flag = true;
                }
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
                    if let Ok(visible_num) = self.visible_num.try_read() {
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
                            .tint(Color32::from_white_alpha((255.0 * *visible_num) as u8))
                            .atom_size(btn_rect);
                        let play_or_pause_btn = egui::Button::new(btn_img)
                            .fill(egui::Color32::from_rgba_unmultiplied(
                                0,
                                0,
                                0,
                                (10.0 * *visible_num) as u8,
                            ))
                            .stroke(Stroke::new(
                                1.0,
                                Color32::from_rgba_unmultiplied(
                                    0,
                                    0,
                                    0,
                                    (10.0 * *visible_num) as u8,
                                ),
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
                            }
                        }
                    }
                });
        }
    }

    // fn paint_subtitle(&mut self, ui: &mut Ui, ctx: &Context) {
    //     ui.horizontal(|ui| {
    //         if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
    //             if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
    //                 ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
    //                     if let Ok(generated_str) = self.subtitle_text_receiver.try_recv() {
    //                         self.subtitle_text.push_str(&generated_str);
    //                     }
    //                     if self.subtitle_text.len() > 50 {
    //                         self.subtitle_text.remove(0);
    //                     }
    //                     if let Ok(used_model) = self.used_model.try_read() {
    //                         if let UsedModel::Empty = &*used_model
    //                             && !self.subtitle_text.is_empty()
    //                         {
    //                             self.subtitle_text.clear();
    //                         }
    //                     }
    //                     let subtitle_text_button = egui::Button::new(
    //                         RichText::new(self.subtitle_text.clone())
    //                             .size(50.0)
    //                             .color(*THEME_COLOR)
    //                             .atom_size(Vec2::new(ctx.content_rect().width(), 10.0)),
    //                     )
    //                     .frame(false);
    //                     let be_opacity = ui.opacity();
    //                     ui.set_opacity(1.0);
    //                     ui.add(subtitle_text_button);
    //                     ui.set_opacity(be_opacity);
    //                 });
    //             }
    //         }
    //     });
    // }

    fn paint_frame_info_text(&self, ui: &mut Ui, now: &Instant) {
        ui.horizontal(|ui| {
            let app_sec = (*now - self.app_start_instant).as_secs();
            let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
            orange_color[3] = 100;
            if let Some(fps) = ui.ctx().cumulative_frame_nr().checked_div(app_sec) {
                let mut text_str = "fps：".to_string();
                text_str.push_str(fps.to_string().as_str());

                let rich_text = egui::RichText::new(text_str)
                    .color(Color32::from_rgba_unmultiplied(
                        orange_color[0],
                        orange_color[1],
                        orange_color[2],
                        orange_color[3],
                    ))
                    .size(30.0);
                let fps_button = egui::Button::new(rich_text).frame(false);
                ui.add(fps_button);
            }
            let mut date_time_str = "date-time：".to_string();
            if let Ok(formatter) =
                time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
            {
                if let Ok(local_date_time) = time::OffsetDateTime::now_local() {
                    if let Ok(formatted_date_time_str) = local_date_time.format(&formatter) {
                        date_time_str.push_str(formatted_date_time_str.as_str());
                    }
                }
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
    fn check_play_end(&self) -> bool {
        if !self
            .ui_flags
            .live_mode
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let pts = self
                .current_main_stream_timestamp
                .load(std::sync::atomic::Ordering::Relaxed);
            let end_ts = self.end_ts.load(std::sync::atomic::Ordering::Relaxed);
            if pts >= end_ts
            // tiny_decoder.end_audio_ts() * audio_time_base.numerator() as i64
            //     / audio_time_base.denominator() as i64
            {
                warn!("play end! end_ts:{end_ts},current_ts:{pts} ");
                return true;
            }
        }
        false
    }

    async fn reset_main_color_img_to_bg(
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
    async fn reset_main_color_img_to_cover(
        tiny_decoder: &TinyDecoder,
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        let cover_pic_data = tiny_decoder.cover_pic_data.clone();
        let cover_data = cover_pic_data.read().await;
        if let Some(data_vec) = &*cover_data {
            if let Ok(img) = image::load_from_memory(data_vec) {
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
    }
    pub fn change_format_input(context: ChangeInputContext) -> PlayerResult<()> {
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
                if let Err(e) = present_data_manager.stop_present_tasks() {
                    warn!("{:?}", e);
                }

                let mut tiny_decoder = context.tiny_decoder.write().await;

                if let Err(e) = tiny_decoder.reset_input(&context.path).await {
                    warn!("{}", e);
                }
                context.audio_player.clear();
                let video_rect = tiny_decoder.video_frame_rect;
                Self::reset_main_color_img_to_bg(
                    context.bg_dyn_img,
                    &video_rect,
                    context.main_color_image.clone(),
                )
                .await;
                Self::reset_main_color_img_to_cover(
                    &tiny_decoder,
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
                    warn!("{}", e);
                }
                info!("reset video texture success");
                present_data_manager.start_present_tasks();
            }
        });

        Ok(())
    }

    // fn map_video_data_to_texture(&mut self, ctx: &Context, frame: &mut Frame) {
    //     if let Ok(current_video_frame) = self.video_frame_recv.try_recv() {
    //         // info!("new frame received");
    //         if let Some(pts) = current_video_frame.pts() {
    //             if pts > 0 {
    //                 self.current_video_timestamp
    //                     .store(pts, std::sync::atomic::Ordering::Release);
    //             }
    //         }
    //         if let Some(v_tex) = &mut self.video_texture {
    //             if current_video_frame.pts().is_some() {
    //                 if let Some(wgpu_render_state) = frame.wgpu_render_state() {
    //                     let renderer = wgpu_render_state.renderer.read();
    //                     if let Some(wgpu_texture) = renderer.texture(&v_tex.id) {
    //                         if let Some(texture) = &wgpu_texture.texture {
    //                             let texel_copy_info = TexelCopyTextureInfo {
    //                                 texture,
    //                                 mip_level: 0,
    //                                 origin: Origin3d::ZERO,
    //                                 aspect: TextureAspect::All,
    //                             };
    //                             unsafe {
    //                                 wgpu_render_state.queue.write_texture(
    //                                     texel_copy_info,
    //                                     current_video_frame.data(0),
    //                                     TexelCopyBufferLayout {
    //                                         offset: 0,
    //                                         bytes_per_row: Some(
    //                                             (*current_video_frame.as_ptr()).linesize[0] as u32,
    //                                         ),
    //                                         rows_per_image: None,
    //                                     },
    //                                     Extent3d {
    //                                         width: current_video_frame.width(),
    //                                         height: current_video_frame.height(),
    //                                         depth_or_array_layers: 1,
    //                                     },
    //                                 );
    //                             }
    //                             ctx.request_repaint();
    //                         }
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
    fn detect_file_drag(&mut self, ui: &mut Ui) {
        let mut detected = None;
        ui.input(|input| {
            let dropped_files = &input.raw.dropped_files;
            if !dropped_files.is_empty() {
                if let Some(path) = &dropped_files[0].path {
                    detected = Some(path.to_path_buf());
                }
            }
        });
        if let Some(path_buf) = detected {
            let mut ctx = self.change_input_context.clone();
            ctx.path = path_buf.clone();
            if Self::change_format_input(ctx).is_ok() {
                if let Some(p_str) = path_buf.to_str() {
                    warn!("filepath{}", p_str);
                }
                self.ui_flags
                    .live_mode
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            } else {
                self.tip_window_msg = "please choose a valid video or audio file !!!".to_string();
                self.ui_flags.tip_window_flag = true;
            }
        }
    }
    fn paint_playlist_button(&mut self, ui: &mut Ui) {
        if let Ok(visible_num) = self.visible_num.try_read() {
            let open_btn = Button::new(
                Image::from(PLAY_LIST_IMG)
                    .tint(Color32::from_white_alpha((255.0 * *visible_num) as u8))
                    .atom_size(Vec2::new(50.0, 50.0)),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                0,
                0,
                0,
                (10.0 * *visible_num) as u8,
            ))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * *visible_num) as u8),
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
                    .tint(Color32::from_white_alpha((255.0 * *visible_num) as u8))
                    .atom_size(Vec2::new(50.0, 50.0)),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                0,
                0,
                0,
                (10.0 * *visible_num) as u8,
            ))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * *visible_num) as u8),
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
                    if let Ok(t) = en.file_type() {
                        if t.is_file() {
                            if let Some(file_name) = en.file_name().to_str() {
                                if file_name.ends_with(".ts")
                                    || file_name.ends_with(".mp4")
                                    || file_name.ends_with(".mkv")
                                    || file_name.ends_with(".flac")
                                    || file_name.ends_with(".mp3")
                                    || file_name.ends_with(".m4a")
                                    || file_name.ends_with(".wav")
                                    || file_name.ends_with(".ogg")
                                    || file_name.ends_with(".opus")
                                {
                                    let media_path = en.path().clone();
                                    let cover = Self::load_file_cover(&media_path).await;
                                    let texture_handle =
                                        Self::load_cover_texture(&ctx, &cover, file_name).await;
                                    video_targets.push(VideoDes {
                                        name: file_name.to_string(),
                                        path: media_path,
                                        texture_handle,
                                    });
                                }
                            }
                        }
                    }
                } else {
                    warn!("read dir element err");
                }
            }
        }
    }
    async fn load_file_cover(file_path: &Path) -> RgbaImage {
        if let Ok(input) = &mut ffmpeg_the_third::format::input(file_path) {
            let mut cover_idx = None;

            for stream in input.streams() {
                if let Type::Video = stream.parameters().medium() {
                    if let Disposition::ATTACHED_PIC = stream.disposition() {
                        cover_idx = Some(stream.index());
                        break;
                    }
                }
            }
            if let Some(idx) = cover_idx {
                for packet in input.packets() {
                    if let Ok((stream, p)) = &packet {
                        if stream.index() == idx {
                            if let Some(cover_data) = p.data() {
                                if let Ok(dyn_img) = image::load_from_memory(cover_data) {
                                    return dyn_img.to_rgba8();
                                }
                            }
                        }
                    }
                }
            }
        }
        if let ImageSource::Bytes { bytes, .. } = PLAY_IMG {
            if let Ok(dyn_img) = image::load_from_memory(bytes.as_bytes()) {
                return dyn_img.to_rgba8();
            }
        }
        RgbaImage::new(1920, 1080)
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
        if self.ui_flags.tip_window_flag {
            let tip_window = egui::Window::new("tip window");
            tip_window.show(ctx, |ui| {
                let tip_text = RichText::new(&self.tip_window_msg).size(20.0);

                ui.add(Button::new(tip_text));
                if ui.button("close").clicked() {
                    self.ui_flags.tip_window_flag = false;
                }
            });
        }
    }
    fn visiable_anime(&mut self, ui: &mut Ui) {
        if let Ok(mut visible_num) = self.visible_num.try_write() {
            let visible_id = ui.make_persistent_id("visiable_num");
            *visible_num = ui.ctx().animate_bool_with_time(
                visible_id,
                self.ui_flags
                    .visible_flag
                    .load(std::sync::atomic::Ordering::Relaxed),
                4.0,
            );
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
#[derive(Clone, Builder)]
pub struct ChangeInputContext {
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
}
