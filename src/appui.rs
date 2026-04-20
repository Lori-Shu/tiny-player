use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicI64},
    },
    time::Instant,
};

use eframe::{
    CreationContext, Frame,
    egui_wgpu::RenderState,
    wgpu::{
        Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
        TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
    },
};
use egui::{
    AtomExt, Button, Color32, ColorImage, Context, CornerRadius, Image, ImageData, ImageSource,
    Layout, Pos2, Rect, RichText, Slider, Stroke, TextureHandle, TextureId, TextureOptions, Ui,
    Vec2, ViewportBuilder, ViewportId, WidgetText, include_image,
};

use ffmpeg_the_third::{format::stream::Disposition, media::Type};
use image::{DynamicImage, EncodableLayout, RgbaImage};

use tokio::{runtime::Runtime, sync::RwLock};
use tracing::{info, warn};

use crate::{
    PlayerResult,
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

/// the main struct stores all the vars which are related to ui
struct UiFlags {
    pause_flag: Arc<AtomicBool>,
    fullscreen_flag: bool,
    tip_window_flag: bool,
    playlist_window_flag: bool,
    show_subtitle_options_flag: bool,
    show_volumn_slider_flag: bool,
    visible_flag: bool,
    media_source_flag: Arc<AtomicBool>,
}

pub struct AppUi {
    video_texture: Arc<RwLock<VideoTextureWithId>>,
    tiny_decoder: Arc<RwLock<crate::decode::TinyDecoder>>,
    audio_player: crate::audio_play::AudioPlayer,
    _present_data_manager: PresentDataManager,
    main_stream_current_timestamp: Arc<AtomicI64>,
    main_color_image: Arc<RwLock<ColorImage>>,
    bg_dyn_img: Arc<DynamicImage>,
    ui_flags: UiFlags,
    play_time: time::Time,
    time_text: String,
    tip_window_msg: String,
    app_start_instant: Instant,
    async_rt: Runtime,
    open_file_dialog: Option<egui_file::FileDialog>,
    scan_folder_dialog: Option<egui_file::FileDialog>,
    // _subtitle: Arc<RwLock<AISubTitle>>,
    // subtitle_text: String,
    // subtitle_text_receiver: mpsc::Receiver<String>,
    video_des: Arc<RwLock<Vec<VideoDes>>>,
    // used_model: Arc<RwLock<UsedModel>>,
    audio_volumn: f32,
    current_video_timestamp: Arc<AtomicI64>,
    visible_num: f32,
}
impl eframe::App for AppUi {
    /// this function will automaticly be called every ui redraw
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical(|ui| {
                /*
                down part is update data part with no ui painting

                 */
                let now = Instant::now();
                {
                    if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
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
                                if self.check_play_is_at_endtail(&tiny_decoder) {
                                    self.ui_flags
                                        .pause_flag
                                        .store(true, std::sync::atomic::Ordering::Release);
                                }
                            }
                        }
                    }
                }
                /*
                down part is ui painting and control

                 */
                self.visiable_anime(ui);
                self.ui_flags.visible_flag = false;
                self.paint_video_image(ui);
                self.paint_frame_info_text(ui, &now);

                ui.horizontal(|ui| {
                    self.paint_tip_window(ui.ctx());
                    self.paint_file_btn(ui, frame);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        self.paint_playlist_button(ui, frame);
                    });
                });

                ui.add_space(ui.ctx().content_rect().height() / 2.0 - 200.0);
                ui.horizontal(|ui| {
                    self.paint_playpause_btn(ui);
                });

                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    self.update_time_and_time_text();
                    self.paint_control_area(ui);
                    // self.paint_subtitle(ui, ctx);
                });

                self.detect_file_drag(ui, frame);
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
        let main_color_image = Arc::new(RwLock::new(color_image));
        let media_source_flag = Arc::new(AtomicBool::new(false));
        let tiny_decoder =
            crate::decode::TinyDecoder::new(rt.clone(), cc, media_source_flag.clone())?;
        let tiny_decoder = Arc::new(RwLock::new(tiny_decoder));
        // let used_model = Arc::new(RwLock::new(UsedModel::Empty));
        // let subtitle_channel = mpsc::channel(10);
        // let subtitle = Arc::new(RwLock::new(AISubTitle::new(subtitle_channel.0)?));
        let audio_player = crate::audio_play::AudioPlayer::new()?;
        let main_stream_current_timestamp = Arc::new(AtomicI64::new(0));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let current_video_timestamp = Arc::new(AtomicI64::new(0));
        let video_texture = Arc::new(RwLock::new(VideoTextureWithId {
            texture: None,
            id: None,
        }));
        let data_manage_context = DataManageContextBuilder::default()
            .tiny_decoder(tiny_decoder.clone())
            // .used_model(used_model.clone())
            // .ai_subtitle(subtitle.clone())
            .video_texture_with_id(video_texture.clone())
            .audio_sink(audio_player.sink())
            .main_stream_current_timestamp(main_stream_current_timestamp.clone())
            .current_video_timestamp(current_video_timestamp.clone())
            .runtime_handle(rt)
            .pause_flag(pause_flag.clone())
            .build()?;
        let present_data_manager = PresentDataManager::new(data_manage_context);
        async_rt.block_on(Self::reset_video_texture(
            main_color_image.clone(),
            video_texture.clone(),
            cc.wgpu_render_state
                .as_ref()
                .ok_or(anyhow::Error::msg("get render state err"))?
                .clone(),
        ))?;
        Ok(Self {
            // subtitle_text_receiver: subtitle_channel.1,
            video_texture,
            tiny_decoder,
            audio_player,
            _present_data_manager: present_data_manager,
            main_stream_current_timestamp,
            play_time,
            main_color_image,
            ui_flags: UiFlags {
                pause_flag,
                fullscreen_flag: false,
                tip_window_flag: false,
                playlist_window_flag: false,
                show_subtitle_options_flag: false,
                show_volumn_slider_flag: false,
                visible_flag: false,
                media_source_flag,
            },
            // used_model,
            time_text: String::new(),

            tip_window_msg: String::new(),
            app_start_instant: Instant::now(),
            async_rt,
            open_file_dialog: Some(f_dialog),
            scan_folder_dialog: Some(egui_file::FileDialog::select_folder()),
            bg_dyn_img: Arc::new(dyn_img),
            // _subtitle: subtitle,
            // subtitle_text: String::new(),
            video_des: Arc::new(RwLock::new(vec![])),
            audio_volumn: 1.0,
            current_video_timestamp,
            visible_num: 1.0,
        })
    }
    fn paint_video_image(&mut self, ui: &mut Ui) {
        /*
        show image that contains the video texture
         */
        let layer_painter = ui.ctx().layer_painter(ui.layer_id());
        if let Ok(video_texture_with_id) = self.video_texture.try_read() {
            if let Some(id) = &video_texture_with_id.id {
                layer_painter.image(
                    *id,
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
    }
    fn update_time_and_time_text(&mut self) {
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
            if self
                .ui_flags
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                let play_ts = self
                    .main_stream_current_timestamp
                    .load(std::sync::atomic::Ordering::Relaxed);
                let sec_num = {
                    if let MainStream::Audio = tiny_decoder.main_stream() {
                        let audio_time_base = tiny_decoder.audio_time_base();
                        play_ts * audio_time_base.numerator() as i64
                            / audio_time_base.denominator() as i64
                    } else {
                        let v_time_base = tiny_decoder.video_time_base();

                        play_ts * v_time_base.numerator() as i64 / v_time_base.denominator() as i64
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

    async fn reset_video_texture(
        main_color_image: Arc<RwLock<ColorImage>>,
        texture_with_id: Arc<RwLock<VideoTextureWithId>>,
        render_state: RenderState,
    ) -> PlayerResult<()> {
        let main_color_image = main_color_image.read().await;
        // let id = {
        //     let video_texture = self.video_texture.blocking_read();
        //     video_texture.id
        // };

        // if let Some(id) = id {
        //     render_state.renderer.write().free_texture(&id);
        // }
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
        {
            let mut texture_with_id = texture_with_id.write().await;
            texture_with_id.texture = Some(video_texture);
            texture_with_id.id = Some(texture_id);
        }

        Ok(())
    }

    fn paint_file_btn(&mut self, ui: &mut Ui, frame: &Frame) {
        let btn_rect = Vec2::new(
            ui.ctx().content_rect().width() / 10.0,
            ui.ctx().content_rect().width() / 10.0,
        );
        let file_image_button = egui::Button::new(
            Image::from(VIDEO_FILE_IMG)
                .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                .atom_size(btn_rect),
        )
        .fill(egui::Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            (10.0 * self.visible_num) as u8,
        ))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let file_img_btn_response = ui.add(file_image_button);

        if file_img_btn_response.hovered() {
            self.ui_flags.visible_flag = true;
        }
        if file_img_btn_response.clicked() {
            if let Some(dialog) = &mut self.open_file_dialog {
                dialog.open();
            }
        }
        let mut file_path = None;
        if let Some(d) = &mut self.open_file_dialog {
            d.show(ui.ctx());
            if d.selected() {
                if let Some(p) = d.path() {
                    warn!("path selected{:#?}", p);
                    file_path = Some(p.to_path_buf())
                }
            }
        }
        if let Some(p) = file_path {
            if self.change_format_input(frame, &p).is_ok() {
                if let Some(p_str) = p.to_str() {
                    warn!("accept file path{}", p_str);
                }
            } else {
                self.tip_window_msg = "please choose a valid video or audio file !!!".to_string();
                self.ui_flags.tip_window_flag = true;
            }
        }
    }

    fn paint_playpause_btn(&mut self, ui: &mut Ui) {
        if self
            .ui_flags
            .media_source_flag
            .load(std::sync::atomic::Ordering::Acquire)
        {
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
                .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                .atom_size(btn_rect);
            let play_or_pause_btn = egui::Button::new(btn_img)
                .fill(egui::Color32::from_rgba_unmultiplied(
                    0,
                    0,
                    0,
                    (10.0 * self.visible_num) as u8,
                ))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
                ))
                .corner_radius(CornerRadius::from(30));

            ui.add_space(ui.ctx().content_rect().width() / 2.0 - 100.0);
            let btn_response = ui.add(play_or_pause_btn);
            if btn_response.hovered() {
                self.ui_flags.visible_flag = true;
            }
            if btn_response.clicked() || ui.ctx().input(|s| s.key_released(egui::Key::Space)) {
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
    }

    fn paint_control_area(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut ts = self
                .main_stream_current_timestamp
                .load(std::sync::atomic::Ordering::Relaxed);
            if self
                .ui_flags
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                let mut slider_color = THEME_COLOR.to_srgba_unmultiplied();
                slider_color[3] = 100;
                ui.scope(|ui| {
                    ui.set_opacity(255.0 * self.visible_num);

                    let progress_slider = if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
                        let end_ts = tiny_decoder.end_ts();
                        egui::Slider::new(&mut ts, 0..=end_ts)
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
                            )))
                    } else {
                        Slider::new(&mut ts, 0..=100)
                    };

                    let mut slider_width_style = egui::style::Style::default();
                    slider_width_style.spacing.slider_width =
                        ui.ctx().content_rect().width() - 450.0;
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
                        self.ui_flags.visible_flag = true;
                    }
                    if slider_response.changed() {
                        warn!("slider dragged!");
                        let audio_player = &mut self.audio_player;
                        let tiny_decoder = self.tiny_decoder.clone();
                        self.async_rt.spawn(async move {
                            let tiny_decoder = tiny_decoder.read().await;
                            tiny_decoder.seek_timestamp_to_decode(ts).await;
                        });

                        audio_player.source_queue_skip_to_end();
                        if !self
                            .ui_flags
                            .pause_flag
                            .load(std::sync::atomic::Ordering::Relaxed)
                        {
                            audio_player.play();
                        }
                    }
                });

                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let subtitle_btn = Button::new(
                        Image::from(SUBTITLE_IMG)
                            .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                            .atom_size(Vec2::new(50.0, 50.0)),
                    )
                    .fill(egui::Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        (10.0 * self.visible_num) as u8,
                    ))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
                    ))
                    .corner_radius(CornerRadius::from(30));
                    let btn_response = ui.add(subtitle_btn);
                    if btn_response.hovered() {
                        self.ui_flags.visible_flag = true;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.show_subtitle_options_flag =
                            !self.ui_flags.show_subtitle_options_flag;
                    }
                    // let used_model = self.used_model.clone();
                    // let mut used_model = self.async_rt.block_on(used_model.write());
                    // if self.ui_flags.show_subtitle_options_flag {
                    //     ui.radio_value(&mut *used_model, UsedModel::Empty, "closed");
                    //     ui.radio_value(&mut *used_model, UsedModel::Chinese, "中文");
                    //     ui.radio_value(&mut *used_model, UsedModel::English, "English");
                    // }
                });
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let volumn_img_btn = egui::Button::new(
                        Image::from(VOLUME_IMG)
                            .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                            .atom_size(Vec2::new(50.0, 50.0)),
                    )
                    .fill(egui::Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        (10.0 * self.visible_num) as u8,
                    ))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
                    ))
                    .corner_radius(CornerRadius::from(30));
                    let btn_response = ui.add(volumn_img_btn);
                    if btn_response.hovered() {
                        self.ui_flags.visible_flag = true;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.show_volumn_slider_flag =
                            !self.ui_flags.show_volumn_slider_flag;
                    }
                    if self.ui_flags.show_volumn_slider_flag {
                        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                            ui.add_space(150.0);
                            let audio_player = &mut self.audio_player;
                            ui.scope(|ui| {
                                ui.set_opacity(255.0 * self.visible_num);
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
                                slider_response = slider_response.on_hover_text(
                                    (audio_player.current_volumn() * 100.0).to_string(),
                                );
                                if slider_response.hovered() {
                                    self.ui_flags.visible_flag = true;
                                }
                                if slider_response.changed() {
                                    warn!("volumn slider dragged!");
                                    audio_player.change_volumn(self.audio_volumn);
                                }
                            });
                        });
                    }
                });
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let fullscreen_image_btn = egui::Button::new(
                        Image::from(FULLSCREEN_IMG)
                            .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                            .atom_size(Vec2::new(50.0, 50.0)),
                    )
                    .fill(egui::Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        (10.0 * self.visible_num) as u8,
                    ))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
                    ))
                    .corner_radius(CornerRadius::from(30));
                    let btn_response = ui.add(fullscreen_image_btn);
                    if btn_response.hovered() {
                        self.ui_flags.visible_flag = true;
                    }
                    if btn_response.clicked() {
                        self.ui_flags.fullscreen_flag = !self.ui_flags.fullscreen_flag;
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                self.ui_flags.fullscreen_flag,
                            ));
                    }
                });
            }
            self.main_stream_current_timestamp
                .store(ts, std::sync::atomic::Ordering::Release);
        });
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
    fn check_play_is_at_endtail(&self, tiny_decoder: &TinyDecoder) -> bool {
        let pts = self
            .main_stream_current_timestamp
            .load(std::sync::atomic::Ordering::Relaxed);
        let main_stream_time_base = {
            if let MainStream::Audio = tiny_decoder.main_stream() {
                tiny_decoder.audio_time_base()
            } else {
                tiny_decoder.video_time_base()
            }
        };
        if pts
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

        false
    }

    /// return true when the current video time > audio time,else return false
    fn paint_playlist_window(&mut self, ui: &mut Ui, frame: &Frame) {
        if self.ui_flags.playlist_window_flag {
            let viewport_id = ViewportId::from_hash_of("content_window");
            ui.ctx()
                .send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Visible(true));
            ui.ctx().send_viewport_cmd_to(
                viewport_id,
                egui::ViewportCommand::Title("content_window".to_string()),
            );

            let viewport_builder = ViewportBuilder::default().with_close_button(false);
            ui.show_viewport_immediate(viewport_id, viewport_builder, |ui, _| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    ui.vertical(|ui| {
                        let video_urls_scroll = egui::ScrollArea::vertical().max_height(500.0);

                        video_urls_scroll.show(ui, |ui| {
                            let video_des = self.video_des.clone();
                            if let Ok(videos) = video_des.try_write() {
                                ui.columns(2, |columns| {
                                    for (i, des) in videos.iter().enumerate() {
                                        let image_btn = Button::new(
                                            Image::new(&des.texture_handle)
                                                .atom_size(Vec2::new(1920.0 / 6.0, 1080.0 / 6.0)),
                                        );

                                        let player_text_button = Button::new(des.name.clone());
                                        if i % 2 == 0 {
                                            columns[0].vertical(|ui| {
                                                ui.add(image_btn);
                                                if ui.add(player_text_button).clicked() {
                                                    if self
                                                        .change_format_input(frame, &des.path)
                                                        .is_ok()
                                                    {
                                                        info!("change_format_input success");
                                                    }
                                                }
                                            });
                                        } else {
                                            columns[1].vertical(|ui| {
                                                ui.add(image_btn);
                                                if ui.add(player_text_button).clicked() {
                                                    match self.change_format_input(frame, &des.path)
                                                    {
                                                        Ok(()) => {
                                                            info!("change_format_input success");
                                                        }
                                                        Err(e) => {
                                                            warn!("{}", e);
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                    }
                                });
                            }
                        });
                        if let Some(dialog) = &mut self.scan_folder_dialog {
                            dialog.show(ui.ctx());
                            if ui.button("scan video folder").clicked() {
                                dialog.open();
                            }
                            if dialog.selected() {
                                {
                                    let mut videos = self.video_des.blocking_write();
                                    videos.clear();
                                }
                                if let Some(path) = dialog.path() {
                                    let video_des = self.video_des.clone();
                                    let path = path.to_path_buf();
                                    let ctx = ui.ctx().clone();
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
    async fn reset_main_color_img_to_bg(
        bg_dyn_img: Arc<DynamicImage>,
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        let bg_color_img = ColorImage::from_rgba_unmultiplied(
            [bg_dyn_img.width() as usize, bg_dyn_img.height() as usize],
            bg_dyn_img.as_bytes(),
        );
        let mut main_color_image = main_color_image.write().await;
        *main_color_image = bg_color_img;
    }
    async fn reset_main_color_img_to_cover_pic(
        tiny_decoder: &TinyDecoder,
        main_color_image: Arc<RwLock<ColorImage>>,
    ) {
        let cover_pic_data = tiny_decoder.cover_pic_data();
        let cover_data = cover_pic_data.read().await;
        if let Some(data_vec) = &*cover_data {
            if let Ok(img) = image::load_from_memory(data_vec) {
                let rgba8_img = img.to_rgba8();
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
    fn change_format_input(&mut self, frame: &Frame, path: &Path) -> PlayerResult<()> {
        self.ui_flags
            .pause_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.main_stream_current_timestamp
            .store(0, std::sync::atomic::Ordering::Release);
        self.current_video_timestamp
            .store(0, std::sync::atomic::Ordering::Release);
        let tiny_decoder = self.tiny_decoder.clone();
        let au_sink = self.audio_player.sink();
        let main_color_img = self.main_color_image.clone();
        let bg_dyn_img = self.bg_dyn_img.clone();
        let texture_with_id = self.video_texture.clone();
        let render_state = frame
            .wgpu_render_state()
            .ok_or(anyhow::Error::msg("get render state from egui::Frame err"))?
            .clone();

        let path = path.to_path_buf();
        self.async_rt.spawn(async move {
            let mut tiny_decoder = tiny_decoder.write().await;
            if let Err(e) = tiny_decoder.set_file_path_and_init_par(&path).await {
                warn!("{}", e);
            }
            au_sink.clear();
            Self::reset_main_color_img_to_bg(bg_dyn_img, main_color_img.clone()).await;
            Self::reset_main_color_img_to_cover_pic(&tiny_decoder, main_color_img.clone()).await;

            if let Err(e) =
                Self::reset_video_texture(main_color_img, texture_with_id, render_state).await
            {
                warn!("{}", e);
            }
            info!("reset video texture success");
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
    fn detect_file_drag(&mut self, ui: &mut Ui, frame: &Frame) {
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
            if self.change_format_input(frame, path_buf.as_path()).is_ok() {
                if let Some(p_str) = path_buf.to_str() {
                    warn!("filepath{}", p_str);
                }
            } else {
                self.tip_window_msg = "please choose a valid video or audio file !!!".to_string();
                self.ui_flags.tip_window_flag = true;
            }
        }
    }
    fn paint_playlist_button(&mut self, ui: &mut Ui, frame: &Frame) {
        let open_btn = Button::new(
            Image::from(PLAY_LIST_IMG)
                .tint(Color32::from_white_alpha((255.0 * self.visible_num) as u8))
                .atom_size(Vec2::new(50.0, 50.0)),
        )
        .fill(egui::Color32::from_rgba_unmultiplied(
            0,
            0,
            0,
            (10.0 * self.visible_num) as u8,
        ))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (10.0 * self.visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.ui_flags.visible_flag = true;
        }
        if btn_response.clicked() {
            self.ui_flags.playlist_window_flag = !self.ui_flags.playlist_window_flag;
        }
        if self.ui_flags.playlist_window_flag {
            self.paint_playlist_window(ui, frame);
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
    fn visiable_anime(&mut self, ui: &mut Ui) {
        let visible_id = ui.make_persistent_id("visiable_num");
        self.visible_num =
            ui.ctx()
                .animate_bool_with_time(visible_id, self.ui_flags.visible_flag, 2.0);
    }
}

struct VideoDes {
    pub name: String,
    pub path: PathBuf,
    pub texture_handle: TextureHandle,
}

pub struct VideoTextureWithId {
    pub texture: Option<Texture>,
    pub id: Option<TextureId>,
}
