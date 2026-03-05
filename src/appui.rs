use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use eframe::{
    Frame,
    wgpu::{Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect},
};
use egui::{
    AtomExt, Button, Color32, ColorImage, Context, Image, ImageData, ImageSource, Layout, Pos2,
    Rect, RichText, TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder, ViewportId, Widget,
    WidgetText, include_image,
};

use ffmpeg_the_third::{format::stream::Disposition, frame::Video, media::Type};
use image::{DynamicImage, EncodableLayout, RgbaImage};

use tokio::{
    runtime::Runtime,
    sync::{
        Notify, RwLock, mpsc,
        watch::{self, Receiver, Sender},
    },
};
use tracing::{info, warn};

use crate::{
    PlayerResult,
    ai_sub_title::{AISubTitle, UsedModel},
    decode::{MainStream, TinyDecoder},
    present_data_manage::{DataManageContextBuilder, PresentDataManager},
};

const VIDEO_FILE_IMG: ImageSource = include_image!("../resources/file-play.png");
const VOLUME_IMG: ImageSource = include_image!("../resources/volume-2.png");
const PLAY_IMG: ImageSource = include_image!("../resources/play.png");
const PAUSE_IMG: ImageSource = include_image!("../resources/pause.png");
const FULLSCREEN_IMG: ImageSource = include_image!("../resources/fullscreen.png");
const DEFAULT_BG_IMG: ImageSource = include_image!("../resources/background.png");
const PLAY_LIST_IMG: ImageSource = include_image!("../resources/list-video.png");
const SUBTITLE_IMG: ImageSource = include_image!("../resources/captions.png");
pub const MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
const EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
static THEME_COLOR: LazyLock<Color32> = LazyLock::new(|| {
    let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
    orange_color[3] = 200;
    Color32::from_rgba_unmultiplied(
        orange_color[0],
        orange_color[1],
        orange_color[2],
        orange_color[3],
    )
});
struct PlayerTextButton {
    text: String,
    font_size: f32,
    frame: bool,
}
impl PlayerTextButton {
    pub fn new(text: impl Into<String>, font_size: f32, frame: bool) -> Self {
        Self {
            text: text.into(),
            font_size,
            frame,
        }
    }
}
impl Widget for PlayerTextButton {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let btn_text = RichText::new(&self.text)
            .color(*THEME_COLOR)
            .size(self.font_size);
        let open_btn = egui::Button::new(btn_text).frame(self.frame);
        ui.add(open_btn)
    }
}
/// the main struct stores all the vars which are related to ui
struct UiFlags {
    pause_flag: (Sender<bool>, Receiver<bool>),
    fullscreen_flag: bool,
    control_ui_flag: bool,
    tip_window_flag: bool,
    playlist_window_flag: bool,
    show_subtitle_options_flag: bool,
    show_volumn_slider_flag: bool,
}

pub struct AppUi {
    video_texture_handle: Option<TextureHandle>,
    tiny_decoder: Arc<RwLock<crate::decode::TinyDecoder>>,
    audio_player: crate::audio_play::AudioPlayer,
    _present_data_manager: PresentDataManager,
    main_stream_current_timestamp: Arc<RwLock<i64>>,
    main_color_image: ColorImage,
    bg_dyn_img: DynamicImage,
    frame_show_instant: Instant,
    ui_flags: UiFlags,
    play_time: time::Time,
    time_text: String,
    tip_window_msg: String,
    last_show_control_ui_instant: Instant,
    app_start_instant: Instant,
    current_video_frame: Arc<RwLock<Video>>,
    async_rt: Runtime,
    opened_file: Option<std::path::PathBuf>,
    open_file_dialog: Option<egui_file::FileDialog>,
    scan_folder_dialog: Option<egui_file::FileDialog>,
    _subtitle: Arc<RwLock<AISubTitle>>,
    subtitle_text: String,
    subtitle_text_receiver: mpsc::Receiver<String>,
    video_des: Arc<RwLock<Vec<VideoDes>>>,
    used_model: Arc<RwLock<UsedModel>>,
    audio_volumn: f32,
    data_thread_notify: Arc<Notify>,
}
impl eframe::App for AppUi {
    /// this function will automaticly be called every ui redraw
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(15));
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                /*
                down part is update data part with no ui painting

                 */
                let now = Instant::now();
                if self.video_texture_handle.is_none() {
                    self.load_video_texture(ctx);
                    self.frame_show_instant = now;
                }
                {
                    if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
                        if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
                            if !*self.ui_flags.pause_flag.1.borrow() {
                                /*
                                if now is next_frame_time or a little beyond get and show a new frame
                                 */
                                if keepawake::Builder::default()
                                    .display(true)
                                    .idle(true)
                                    .app_name("tiny_player")
                                    .reason("video play")
                                    .create()
                                    .is_err()
                                {
                                    warn!("keep awake err");
                                }
                                self.notify_data_thread(&tiny_decoder);
                                if self.check_play_is_at_endtail(&tiny_decoder) {
                                    if self.ui_flags.pause_flag.0.send(true).is_err() {
                                        warn!("change pause flag err");
                                    }
                                }
                            }
                        }
                    }
                }
                self.copy_video_data_to_texture(frame);
                /*
                down part is ui painting and control

                 */
                self.paint_video_image(ctx, ui);
                self.paint_frame_info_text(ui, ctx, &now);
                if self.ui_flags.control_ui_flag {
                    ui.set_opacity(1.0);
                } else {
                    ui.set_opacity(0.0);
                }
                ui.horizontal(|ui| {
                    self.paint_tip_window(ctx);
                    self.paint_file_btn(ui, ctx, &now);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        self.paint_playlist_button(ui, ctx, &now);
                    });
                });

                ui.add_space(ctx.content_rect().height() / 2.0 - 200.0);
                ui.horizontal(|ui| {
                    self.paint_playpause_btn(ui, ctx, &now);
                });

                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    self.update_time_and_time_text();
                    self.paint_control_area(ui, ctx, &now);
                    self.paint_subtitle(ui, ctx);
                });
                if (now - self.last_show_control_ui_instant).as_secs() > 3 {
                    self.ui_flags.control_ui_flag = false;
                }
                self.detect_file_drag(ctx, &now);
            });
        });
    }
}
impl AppUi {
    pub fn replace_fonts(&self, ctx: &egui::Context) {
        // Start with the default fonts (we will be adding to them rather than replacing them).
        let mut fonts = egui::FontDefinitions::default();

        // Install my own font (maybe supporting non-latin characters).
        // .ttf and .otf files supported.
        fonts.font_data.insert(
            "app_default_font".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(MAPLE_FONT)),
        );
        // 彩色 Emoji 字体
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

        // 设置 fallback 顺序
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(1, "noto_emoji".to_owned());
        // Tell egui to use these fonts:
        ctx.set_fonts(fonts);
    }
    pub fn new() -> PlayerResult<Self> {
        let play_time = time::Time::from_hms(0, 0, 0)?;

        let async_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let rt = async_rt.handle().clone();
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

        let tiny_decoder = crate::decode::TinyDecoder::new(rt.clone())?;
        let tiny_decoder = Arc::new(RwLock::new(tiny_decoder));
        let used_model = Arc::new(RwLock::new(UsedModel::Empty));
        let subtitle_channel = mpsc::channel(10);
        let subtitle = Arc::new(RwLock::new(AISubTitle::new(subtitle_channel.0)?));
        let audio_player = crate::audio_play::AudioPlayer::new()?;
        let empty_frame = Video::empty();
        let current_video_frame = Arc::new(RwLock::new(empty_frame));
        let main_stream_current_timestamp = Arc::new(RwLock::new(0));
        let pause_flag = watch::channel(true);

        let data_thread_notify = Arc::new(Notify::new());
        let data_manage_context = DataManageContextBuilder::default()
            .data_thread_notify(data_thread_notify.clone())
            .tiny_decoder(tiny_decoder.clone())
            .used_model(used_model.clone())
            .ai_subtitle(subtitle.clone())
            .current_video_frame(current_video_frame.clone())
            .audio_sink(audio_player.sink())
            .main_stream_current_timestamp(main_stream_current_timestamp.clone())
            .runtime_handle(rt)
            .build()?;
        let present_data_manager = PresentDataManager::new(data_manage_context);

        Ok(Self {
            subtitle_text_receiver: subtitle_channel.1,
            video_texture_handle: None,
            tiny_decoder,
            audio_player,
            _present_data_manager: present_data_manager,
            main_stream_current_timestamp,
            play_time,
            main_color_image: color_image,
            frame_show_instant: Instant::now(),
            ui_flags: UiFlags {
                pause_flag,
                fullscreen_flag: false,
                control_ui_flag: true,
                tip_window_flag: false,
                playlist_window_flag: false,
                show_subtitle_options_flag: false,
                show_volumn_slider_flag: false,
            },
            used_model,

            time_text: String::new(),

            tip_window_msg: String::new(),

            last_show_control_ui_instant: Instant::now(),
            app_start_instant: Instant::now(),
            current_video_frame,
            async_rt,
            opened_file: None,
            open_file_dialog: Some(f_dialog),
            scan_folder_dialog: Some(egui_file::FileDialog::select_folder()),
            bg_dyn_img: dyn_img,
            _subtitle: subtitle,
            subtitle_text: String::new(),
            video_des: Arc::new(RwLock::new(vec![])),
            audio_volumn: 1.0,
            data_thread_notify,
        })
    }
    fn paint_video_image(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        /*
        show image that contains the video texture
         */
        let layer_painter = ctx.layer_painter(ui.layer_id());
        if let Some(video_texture_handle) = &self.video_texture_handle {
            layer_painter.image(
                video_texture_handle.id(),
                Rect::from_min_max(
                    Pos2::new(0.0, 0.0),
                    Pos2::new(ctx.content_rect().width(), ctx.content_rect().height()),
                ),
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    fn update_time_and_time_text(&mut self) {
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
            if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
                if let Ok(play_ts) = self.main_stream_current_timestamp.try_read() {
                    let sec_num = {
                        if let MainStream::Audio = tiny_decoder.main_stream() {
                            let audio_time_base = tiny_decoder.audio_time_base();
                            *play_ts * audio_time_base.numerator() as i64
                                / audio_time_base.denominator() as i64
                        } else {
                            let v_time_base = tiny_decoder.video_time_base();

                            *play_ts * v_time_base.numerator() as i64
                                / v_time_base.denominator() as i64
                        }
                    };
                    let sec = (sec_num % 60) as u8;
                    let min_num = sec_num / 60;
                    let min = (min_num % 60) as u8;
                    let hour_num = min_num / 60;
                    let hour = hour_num as u8;
                    if let Ok(cur_time) = time::Time::from_hms(hour, min, sec) {
                        if !cur_time.eq(&self.play_time) {
                            if let Ok(formatter) =
                                time::format_description::parse("[hour]:[minute]:[second]")
                            {
                                if let Ok(mut now_str) = cur_time.format(&formatter) {
                                    now_str.push('|');
                                    now_str.push_str(tiny_decoder.end_time_formatted_string());
                                    self.time_text = now_str;
                                    self.play_time = cur_time;
                                }
                            }
                        }
                    } else {
                        warn!("update time str err!");
                    }
                }
            }
        }
    }
    fn update_color_image(&mut self) {
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
            let frame_rect = tiny_decoder.video_frame_rect();
            if frame_rect[0] != 0 {
                let color_image = ColorImage::filled(
                    [frame_rect[0] as usize, frame_rect[1] as usize],
                    Color32::from_rgba_unmultiplied(0, 0, 0, 255),
                );

                self.main_color_image = color_image;
            }
        }
    }
    fn load_video_texture(&mut self, ctx: &egui::Context) {
        /*
        从image背景图加载视频显示用texture
         */
        let main_color_image = &self.main_color_image;
        let t = ctx.load_texture(
            "video_texture",
            ImageData::Color(Arc::new(main_color_image.clone())),
            TextureOptions::LINEAR,
        );
        self.video_texture_handle = Some(t);
    }

    fn paint_file_btn(&mut self, ui: &mut Ui, ctx: &Context, now: &Instant) {
        let btn_rect = Vec2::new(
            ctx.content_rect().width() / 10.0,
            ctx.content_rect().width() / 10.0,
        );
        let file_image_button = egui::Button::new(VIDEO_FILE_IMG.atom_size(btn_rect)).frame(false);

        let file_img_btn_response = ui.add(file_image_button);

        if file_img_btn_response.hovered() {
            self.ui_flags.control_ui_flag = true;
            self.last_show_control_ui_instant = *now;
        }
        if file_img_btn_response.clicked() {
            if let Some(dialog) = &mut self.open_file_dialog {
                dialog.open();
            }
        }

        if let Some(d) = &mut self.open_file_dialog {
            d.show(ctx);
            if d.selected() {
                if let Some(p) = d.path() {
                    warn!("path selected{:#?}", p);
                    self.opened_file = Some(p.to_path_buf());
                }
            }
        }

        if let Some(path) = self.opened_file.take() {
            if self.change_format_input(path.as_path(), now).is_ok() {
                if let Some(p_str) = path.to_str() {
                    warn!("accept file path{}", p_str);
                }
            } else {
                self.tip_window_msg = "please choose a valid video or audio file !!!".to_string();
                self.ui_flags.tip_window_flag = true;
            }
        }
    }

    fn paint_playpause_btn(&mut self, ui: &mut Ui, ctx: &Context, now: &Instant) {
        let decoder = self.tiny_decoder.clone();
        let tiny_decoder = self.async_rt.block_on(decoder.read());
        if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
            let play_or_pause_image_source = if *self.ui_flags.pause_flag.1.borrow() {
                PLAY_IMG
            } else {
                PAUSE_IMG
            };
            let btn_rect = Vec2::new(
                ctx.content_rect().width() / 10.0,
                ctx.content_rect().width() / 10.0,
            );
            let btn_atom = play_or_pause_image_source.atom_size(btn_rect);
            let play_or_pause_btn = egui::Button::new(btn_atom).frame(false);

            ui.add_space(ctx.content_rect().width() / 2.0 - 100.0);
            let btn_response = ui.add_sized(Vec2::new(100.0, 100.0), play_or_pause_btn);
            if btn_response.hovered() {
                self.ui_flags.control_ui_flag = true;
                self.last_show_control_ui_instant = *now;
            }
            if btn_response.clicked() || ctx.input(|s| s.key_released(egui::Key::Space)) {
                let pause_flag = &self.ui_flags.pause_flag;
                let previous_v = *pause_flag.1.borrow();
                if pause_flag.0.send(!previous_v).is_err() {
                    warn!("change pause flag err");
                }
                let audio_player = &self.audio_player;
                if *pause_flag.1.borrow() {
                    audio_player.pause();
                } else {
                    audio_player.play();
                }
            }
        }
    }

    fn paint_control_area(&mut self, ui: &mut Ui, ctx: &Context, now: &Instant) {
        ui.horizontal(|ui| {
            let decoder = self.tiny_decoder.clone();
            let tiny_decoder = self.async_rt.block_on(decoder.read());
            if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
                let mut timestamp = self
                    .async_rt
                    .block_on(self.main_stream_current_timestamp.write());
                let mut slider_color = THEME_COLOR.to_srgba_unmultiplied();
                slider_color[3] = 100;
                let progress_slider = egui::Slider::new(&mut *timestamp, 0..=tiny_decoder.end_ts())
                    .show_value(false)
                    .text(WidgetText::RichText(Arc::new(
                        RichText::new(self.time_text.clone()).size(20.0).color(
                            Color32::from_rgba_unmultiplied(
                                slider_color[0],
                                slider_color[1],
                                slider_color[2],
                                slider_color[3],
                            ),
                        ),
                    )));
                let mut slider_width_style = egui::style::Style::default();
                slider_width_style.spacing.slider_width = ctx.content_rect().width() - 450.0;
                slider_width_style.spacing.slider_rail_height = 10.0;
                slider_width_style.spacing.interact_size = Vec2::new(20.0, 20.0);
                slider_width_style.visuals.extreme_bg_color =
                    Color32::from_rgba_unmultiplied(0, 0, 0, 100);
                slider_width_style.visuals.selection.bg_fill =
                    Color32::from_rgba_unmultiplied(0, 0, 0, 100);
                slider_width_style.visuals.widgets.active.bg_fill =
                    Color32::from_rgba_unmultiplied(0, 0, 0, 100);
                slider_width_style.visuals.widgets.inactive.bg_fill =
                    Color32::from_rgba_unmultiplied(255, 165, 0, 100);
                ui.set_style(slider_width_style);
                let slider_response = ui.add(progress_slider);
                if slider_response.hovered() {
                    self.ui_flags.control_ui_flag = true;
                    self.last_show_control_ui_instant = Instant::now();
                }
                if slider_response.changed() {
                    warn!("slider dragged!");
                    let audio_player = &mut self.audio_player;

                    tiny_decoder.seek_timestamp_to_decode(*timestamp);
                    audio_player.source_queue_skip_to_end();
                    if !*self.ui_flags.pause_flag.1.borrow() {
                        audio_player.play();
                    }
                    self.frame_show_instant = *now;
                    let cur_v_frame = self.current_video_frame.clone();
                    let mut current_video_frame = self.async_rt.block_on(cur_v_frame.write());
                    let empty_frame = Video::empty();
                    *current_video_frame = empty_frame;
                }
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let subtitle_btn =
                        Button::new(SUBTITLE_IMG.atom_size(Vec2::new(50.0, 50.0))).frame(false);
                    let btn_response = ui.add(subtitle_btn);
                    if btn_response.hovered() {
                        self.ui_flags.control_ui_flag = true;
                        self.last_show_control_ui_instant = *now;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.show_subtitle_options_flag =
                            !self.ui_flags.show_subtitle_options_flag;
                    }
                    let used_model = self.used_model.clone();
                    let mut used_model = self.async_rt.block_on(used_model.write());
                    if self.ui_flags.show_subtitle_options_flag {
                        ui.radio_value(&mut *used_model, UsedModel::Empty, "closed");
                        ui.radio_value(&mut *used_model, UsedModel::Chinese, "中文");
                        ui.radio_value(&mut *used_model, UsedModel::English, "English");
                    }
                });
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let volumn_img_btn =
                        egui::Button::new(VOLUME_IMG.atom_size(Vec2::new(50.0, 50.0))).frame(false);
                    let btn_response = ui.add(volumn_img_btn);
                    if btn_response.hovered() {
                        self.ui_flags.control_ui_flag = true;
                        self.last_show_control_ui_instant = *now;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.show_volumn_slider_flag =
                            !self.ui_flags.show_volumn_slider_flag;
                    }
                    if self.ui_flags.show_volumn_slider_flag {
                        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                            ui.add_space(150.0);
                            let audio_player = &mut self.audio_player;
                            let volumn_slider =
                                egui::Slider::new(&mut self.audio_volumn, 0.0..=2.0)
                                    .vertical()
                                    .show_value(false);
                            let mut slider_style = egui::style::Style::default();
                            slider_style.spacing.slider_width = 150.0;
                            slider_style.spacing.slider_rail_height = 10.0;
                            slider_style.spacing.interact_size = Vec2::new(20.0, 20.0);
                            slider_style.visuals.extreme_bg_color =
                                Color32::from_rgba_unmultiplied(0, 0, 0, 100);
                            slider_style.visuals.selection.bg_fill =
                                Color32::from_rgba_unmultiplied(0, 0, 0, 100);
                            slider_style.visuals.widgets.active.bg_fill =
                                Color32::from_rgba_unmultiplied(0, 0, 100, 100);
                            slider_style.visuals.widgets.inactive.bg_fill =
                                Color32::from_rgba_unmultiplied(255, 165, 0, 100);
                            ui.set_style(slider_style);
                            let mut slider_response = ui.add(volumn_slider);
                            slider_response = slider_response
                                .on_hover_text((audio_player.current_volumn() * 100.0).to_string());
                            if slider_response.hovered() {
                                self.ui_flags.control_ui_flag = true;
                                self.last_show_control_ui_instant = *now;
                            }
                            if slider_response.changed() {
                                warn!("volumn slider dragged!");
                                audio_player.change_volumn(self.audio_volumn);
                            }
                        });
                    }
                });
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let fullscreen_image_btn =
                        egui::Button::new(FULLSCREEN_IMG.atom_size(Vec2::new(50.0, 50.0)))
                            .frame(false);
                    let btn_response = ui.add(fullscreen_image_btn);
                    if btn_response.hovered() {
                        self.ui_flags.control_ui_flag = true;
                        self.last_show_control_ui_instant = *now;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.fullscreen_flag = !self.ui_flags.fullscreen_flag;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                            self.ui_flags.fullscreen_flag,
                        ));
                    }
                });
            }
        });
    }
    fn paint_subtitle(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.horizontal(|ui| {
            if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
                if self.async_rt.block_on(tiny_decoder.is_input_exist()) {
                    ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                        if let Ok(generated_str) = self.subtitle_text_receiver.try_recv() {
                            self.subtitle_text.push_str(&generated_str);
                        }
                        if self.subtitle_text.len() > 50 {
                            self.subtitle_text.remove(0);
                        }
                        if let Ok(used_model) = self.used_model.try_read() {
                            if let UsedModel::Empty = &*used_model
                                && !self.subtitle_text.is_empty()
                            {
                                self.subtitle_text.clear();
                            }
                        }
                        let subtitle_text_button = egui::Button::new(
                            RichText::new(self.subtitle_text.clone())
                                .size(50.0)
                                .color(*THEME_COLOR)
                                .atom_size(Vec2::new(ctx.content_rect().width(), 10.0)),
                        )
                        .frame(false);
                        let be_opacity = ui.opacity();
                        ui.set_opacity(1.0);
                        ui.add(subtitle_text_button);
                        ui.set_opacity(be_opacity);
                    });
                }
            }
        });
    }

    fn paint_frame_info_text(&self, ui: &mut Ui, ctx: &Context, now: &Instant) {
        ui.horizontal(|ui| {
            let app_sec = (*now - self.app_start_instant).as_secs();
            let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
            orange_color[3] = 100;
            if app_sec > 0 {
                let mut text_str = "fps：".to_string();
                text_str.push_str((ctx.cumulative_frame_nr() / app_sec).to_string().as_str());

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
    fn check_play_is_at_endtail(&self, tiny_decoder: &TinyDecoder) -> bool {
        if let Ok(pts) = self.main_stream_current_timestamp.try_read() {
            let main_stream_time_base = {
                if let MainStream::Audio = tiny_decoder.main_stream() {
                    tiny_decoder.audio_time_base()
                } else {
                    tiny_decoder.video_time_base()
                }
            };
            if *pts
                + main_stream_time_base.denominator() as i64
                    / main_stream_time_base.numerator() as i64
                    / 2
                >= tiny_decoder.end_ts()
            // tiny_decoder.end_audio_ts() * audio_time_base.numerator() as i64
            //     / audio_time_base.denominator() as i64
            {
                let end = tiny_decoder.end_ts();
                warn!("play end! end_ts:{end},current_ts:{pts} ");
                return true;
            }
        }
        false
    }

    fn _set_current_play_pts(&self, ts: i64) {
        {
            let mut cur_pts = self
                .async_rt
                .block_on(self.main_stream_current_timestamp.write());
            {
                *cur_pts = ts;
            }
        }
    }
    /// return true when the current video time > audio time,else return false
    fn paint_playlist_window(&mut self, ctx: &Context, now: &Instant) {
        if self.ui_flags.playlist_window_flag {
            let viewport_id = ViewportId::from_hash_of("content_window");
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(
                viewport_id,
                egui::ViewportCommand::Title("content_window".to_string()),
            );

            let viewport_builder = ViewportBuilder::default().with_close_button(false);
            ctx.show_viewport_immediate(viewport_id, viewport_builder, |ctx, _| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical(|ui| {
                        let video_urls_scroll = egui::ScrollArea::vertical().max_height(500.0);

                        video_urls_scroll.show(ui, |ui| {
                            let video_des = self.video_des.clone();
                            if let Ok(videos) = video_des.try_write() {
                                ui.columns(2, |columns| {
                                    for (i, des) in videos.iter().enumerate() {
                                        let image = Image::new(&des.texture_handle)
                                            .max_size(Vec2::new(200.0, 180.0));

                                        let player_text_button =
                                            PlayerTextButton::new(des.name.clone(), 20.0, true);
                                        if i % 2 == 0 {
                                            columns[0].vertical(|ui| {
                                                ui.add(image);
                                                if ui.add(player_text_button).clicked() {
                                                    if self
                                                        .change_format_input(
                                                            &PathBuf::from(&des.path),
                                                            now,
                                                        )
                                                        .is_ok()
                                                    {
                                                        info!("change_format_input success");
                                                    }
                                                }
                                            });
                                        } else {
                                            columns[1].vertical(|ui| {
                                                ui.add(image);
                                                if ui.add(player_text_button).clicked() {
                                                    if self
                                                        .change_format_input(
                                                            &PathBuf::from(&des.path),
                                                            now,
                                                        )
                                                        .is_ok()
                                                    {
                                                        info!("change_format_input success");
                                                    }
                                                }
                                            });
                                        }
                                    }
                                });
                            }
                        });
                        if let Some(dialog) = &mut self.scan_folder_dialog {
                            dialog.show(ctx);
                            if ui.button("scan video folder").clicked() {
                                dialog.open();
                            }
                            if dialog.selected() {
                                {
                                    let video_des_arc = self.video_des.clone();
                                    let mut videos = self.async_rt.block_on(video_des_arc.write());
                                    videos.clear();
                                }
                                if let Some(path) = dialog.path() {
                                    let video_des = self.video_des.clone();
                                    let path = path.to_path_buf();
                                    let ctx = ctx.clone();
                                    self.async_rt
                                        .spawn(AppUi::read_video_folder(ctx, path, video_des));
                                }
                            }
                        }
                    });
                });
            });
        }
    }
    fn reset_main_tex_to_bg(&mut self) {
        let bg_color_img = ColorImage::from_rgba_unmultiplied(
            [
                self.bg_dyn_img.width() as usize,
                self.bg_dyn_img.height() as usize,
            ],
            self.bg_dyn_img.as_bytes(),
        );
        self.main_color_image = bg_color_img.clone();
    }
    fn reset_main_tex_to_cover_pic(&mut self) {
        let decoder = self.tiny_decoder.clone();
        let tiny_decoder = self.async_rt.block_on(decoder.read());
        let pic_data = tiny_decoder.cover_pic_data();
        let cover_data = self.async_rt.block_on(pic_data.read());
        if let Some(data_vec) = &*cover_data {
            if let Ok(img) = image::load_from_memory(data_vec) {
                let rgba8_img = img.to_rgba8();
                let cover_color_img = ColorImage::from_rgba_unmultiplied(
                    [rgba8_img.width() as usize, rgba8_img.height() as usize],
                    &rgba8_img,
                );
                info!("set cover img!");
                self.main_color_image = cover_color_img.clone();
            }
        }
    }
    fn change_format_input(&mut self, path: &Path, now: &Instant) -> PlayerResult<()> {
        {
            let decoder = self.tiny_decoder.clone();
            let mut tiny_decoder = self.async_rt.block_on(decoder.write());
            if self.ui_flags.pause_flag.0.send(true).is_err() {
                warn!("change pause flag err");
                return Err(anyhow::Error::msg("change pause flag err"));
            }
            if self
                .async_rt
                .block_on(tiny_decoder.set_file_path_and_init_par(path))
                .is_err()
            {
                warn!("reset file path error!");
                return Err(anyhow::Error::msg("change pause flag err"));
            }
        }
        let au_pl = &mut self.audio_player;
        au_pl.source_queue_skip_to_end();

        self.reset_main_tex_to_bg();
        self.reset_main_tex_to_cover_pic();
        self.update_color_image();
        self.async_rt.block_on(async {
            let mut mutex_guard = self.main_stream_current_timestamp.write().await;
            {
                *mutex_guard = 0;
            }
        });
        let current_video_frame = self.current_video_frame.clone();
        let mut current_video_frame = self.async_rt.block_on(current_video_frame.write());
        let empty_frame = Video::empty();
        *current_video_frame = empty_frame;
        self.frame_show_instant = *now;

        Ok(())
    }

    fn copy_video_data_to_texture(&mut self, frame: &mut Frame) {
        let c_img = &mut self.main_color_image;
        if let Ok(current_video_frame) = self.current_video_frame.try_read() {
            if let Some(v_tex) = &mut self.video_texture_handle {
                if current_video_frame.pts().is_some() {
                    if let Some(wgpu_render_state) = frame.wgpu_render_state() {
                        let renderer = wgpu_render_state.renderer.read();
                        if let Some(wgpu_texture) = renderer.texture(&v_tex.id()) {
                            if let Some(texture) = &wgpu_texture.texture {
                                let texel_copy_info = TexelCopyTextureInfo {
                                    texture,
                                    mip_level: 0,
                                    origin: Origin3d::ZERO,
                                    aspect: TextureAspect::All,
                                };
                                unsafe {
                                    wgpu_render_state.queue.write_texture(
                                        texel_copy_info,
                                        current_video_frame.data(0),
                                        TexelCopyBufferLayout {
                                            offset: 0,
                                            bytes_per_row: Some(
                                                (*current_video_frame.as_ptr()).linesize[0] as u32,
                                            ),
                                            rows_per_image: None,
                                        },
                                        Extent3d {
                                            width: current_video_frame.width(),
                                            height: current_video_frame.height(),
                                            depth_or_array_layers: 1,
                                        },
                                    );
                                }
                            }
                        }
                    }
                } else {
                    v_tex.set(
                        ImageData::Color(Arc::new(c_img.clone())),
                        TextureOptions::LINEAR,
                    );
                }
            }
        }
    }
    fn detect_file_drag(&mut self, ctx: &Context, now: &Instant) {
        ctx.input(|input| {
            let dropped_files = &input.raw.dropped_files;
            if !dropped_files.is_empty() {
                if let Some(path) = &dropped_files[0].path {
                    if self.change_format_input(path.as_path(), now).is_ok() {
                        if let Some(p_str) = path.to_str() {
                            warn!("filepath{}", p_str);
                        }
                    } else {
                        self.tip_window_msg =
                            "please choose a valid video or audio file !!!".to_string();
                        self.ui_flags.tip_window_flag = true;
                    }
                }
            }
        });
    }
    fn paint_playlist_button(&mut self, ui: &mut Ui, ctx: &Context, now: &Instant) {
        let open_btn = Button::new(PLAY_LIST_IMG.atom_size(Vec2::new(50.0, 50.0))).frame(false);

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.ui_flags.control_ui_flag = true;
            self.last_show_control_ui_instant = *now;
        }
        if btn_response.clicked() {
            self.ui_flags.playlist_window_flag = !self.ui_flags.playlist_window_flag;
        }
        if self.ui_flags.playlist_window_flag {
            self.paint_playlist_window(ctx, now);
        }
    }
    async fn read_video_folder(ctx: Context, path: PathBuf, video_des: Arc<RwLock<Vec<VideoDes>>>) {
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
                                    let cover = Self::load_file_cover_pic(&media_path).await;
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
    async fn load_file_cover_pic(file_path: &Path) -> RgbaImage {
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
    async fn load_cover_texture(ctx: &Context, cover: &RgbaImage, name: &str) -> TextureHandle {
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

    fn notify_data_thread(&self, tiny_decoder: &TinyDecoder) {
        if let MainStream::Video = tiny_decoder.main_stream() {
            self.data_thread_notify.notify_one();
        } else {
            if self.audio_player.len() < 10 {
                self.data_thread_notify.notify_one();
            }
        }
    }
    fn paint_tip_window(&mut self, ctx: &Context) {
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
}

struct VideoDes {
    pub name: String,
    pub path: PathBuf,
    pub texture_handle: TextureHandle,
}
