use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicU32},
};

use egui::{
    AtomExt, Button, Color32, CornerRadius, Image, Layout, RichText, Stroke, Ui, Vec2, WidgetText,
};
use tokio::{runtime::Handle, sync::RwLock};
use tracing::info;

use crate::{
    appui::{FULLSCREEN_IMG, SUBTITLE_IMG, THEME_COLOR, VOLUME_IMG},
    audio_play::AudioPlayer,
    decode::TinyDecoder,
};

pub struct ControlBarUI {
    current_main_stream_timestamp: Arc<AtomicI64>,
    media_source_flag: Arc<AtomicBool>,
    visible_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
    end_ts: Arc<AtomicI64>,
    pub time_text: String,
    audio_player: Arc<AudioPlayer>,
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    async_rt: Handle,
    pause_flag: Arc<AtomicBool>,
    show_subtitle_options_flag: bool,
    visible_num: Arc<AtomicU32>,
    audio_volume: f32,
    fullscreen_flag: bool,
    show_volume_slider_flag: bool,
}
impl ControlBarUI {
    pub fn new(
        current_main_stream_timestamp: Arc<AtomicI64>,
        media_source_flag: Arc<AtomicBool>,
        visible_flag: Arc<AtomicBool>,
        live_mode: Arc<AtomicBool>,
        end_ts: Arc<AtomicI64>,
        audio_player: Arc<AudioPlayer>,
        tiny_decoder: Arc<RwLock<TinyDecoder>>,
        async_rt: Handle,
        pause_flag: Arc<AtomicBool>,
        visible_num: Arc<AtomicU32>,
    ) -> Self {
        let time_text = String::new();
        let audio_volume = 1.0_f32;
        let fullscreen_flag = false;
        let show_volume_slider_flag = false;
        let show_subtitle_options_flag = false;
        Self {
            current_main_stream_timestamp,
            media_source_flag,
            visible_flag,
            live_mode,
            end_ts,
            time_text,
            audio_player,
            tiny_decoder,
            async_rt,
            pause_flag,
            show_subtitle_options_flag,
            visible_num,
            audio_volume,
            fullscreen_flag,
            show_volume_slider_flag,
        }
    }
    pub fn paint_controlbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
            {
                self.paint_progress_slider(ui);
                self.paint_caption_button(ui);
                self.paint_volume_button(ui);
                self.paint_fullscreen_button(ui);
            }
        });
    }
    fn paint_progress_slider(&mut self, ui: &mut Ui) {
        let mut slider_color = THEME_COLOR.to_srgba_unmultiplied();
        slider_color[3] = 255;
        ui.scope(|ui| {
            let visible_num =
                f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
            ui.set_opacity(visible_num);

            let (mut ts, end_ts) = if !self.live_mode.load(std::sync::atomic::Ordering::Relaxed) {
                let ts = self
                    .current_main_stream_timestamp
                    .load(std::sync::atomic::Ordering::Relaxed);
                let end_ts = self.end_ts.load(std::sync::atomic::Ordering::Relaxed);
                (ts, end_ts)
            } else {
                (0, 0)
            };
            let progress_slider = egui::Slider::new(&mut ts, 0..=end_ts)
                .show_value(false)
                .text(WidgetText::RichText(Arc::new(
                    RichText::new(self.time_text.clone()).size(22.0).color(
                        Color32::from_rgba_unmultiplied(
                            slider_color[0],
                            slider_color[1],
                            slider_color[2],
                            slider_color[3],
                        ),
                    ),
                )));

            let mut slider_width_style = egui::style::Style::default();
            slider_width_style.spacing.slider_width = ui.ctx().content_rect().width() - 450.0;
            slider_width_style.spacing.slider_rail_height = 10.0;
            slider_width_style.spacing.interact_size = Vec2::new(20.0, 20.0);
            slider_width_style.visuals.extreme_bg_color =
                Color32::from_rgba_unmultiplied(0, 0, 0, 100);
            slider_width_style.visuals.selection.bg_fill =
                Color32::from_rgba_unmultiplied(0, 0, 0, 200);
            slider_width_style.visuals.widgets.active.bg_fill =
                Color32::from_rgba_unmultiplied(0, 0, 0, 200);
            slider_width_style.visuals.widgets.inactive.bg_fill =
                Color32::from_rgba_unmultiplied(255, 165, 0, 200);
            ui.set_style(slider_width_style);
            let slider_response = ui.add(progress_slider);
            if slider_response.hovered() {
                self.visible_flag
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if slider_response.drag_stopped() {
                info!("slider dragged!");
                let audio_player = &mut self.audio_player;
                let tiny_decoder = self.tiny_decoder.clone();
                self.async_rt.spawn(async move {
                    let tiny_decoder = tiny_decoder.read().await;
                    tiny_decoder.seek_timestamp_to_decode(ts).await;
                });

                audio_player.clear_source_queue();
                if !self.pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    audio_player.play();
                }
            }

            self.current_main_stream_timestamp
                .store(ts, std::sync::atomic::Ordering::Release);
        });
    }
    fn paint_caption_button(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            let visible_num =
                f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
            let subtitle_btn = Button::new(
                Image::from(SUBTITLE_IMG)
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
            let btn_response = ui.add(subtitle_btn);
            if btn_response.hovered() {
                self.visible_flag
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if btn_response.clicked() {
                self.show_subtitle_options_flag = !self.show_subtitle_options_flag;
            }

            // let used_model = self.used_model.clone();
            // let mut used_model = self.async_rt.block_on(used_model.write());
            // if self.ui_flags.show_subtitle_options_flag {
            //     ui.radio_value(&mut *used_model, UsedModel::Empty, "closed");
            //     ui.radio_value(&mut *used_model, UsedModel::Chinese, "中文");
            //     ui.radio_value(&mut *used_model, UsedModel::English, "English");
            // }
        });
    }
    fn paint_volume_button(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            let visible_num =
                f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
            let volumn_img_btn = egui::Button::new(
                Image::from(VOLUME_IMG)
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
            let btn_response = ui.add(volumn_img_btn);
            if btn_response.hovered() {
                self.visible_flag
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if btn_response.clicked() {
                self.show_volume_slider_flag = !self.show_volume_slider_flag;
            }
            if self.show_volume_slider_flag {
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let visible_num =
                        f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    ui.add_space(150.0);
                    let audio_player = &mut self.audio_player;
                    ui.scope(|ui| {
                        ui.set_opacity(visible_num);
                        let volumn_slider = egui::Slider::new(&mut self.audio_volume, 0.0..=2.0)
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
                        slider_response =
                            slider_response.on_hover_text((self.audio_volume * 100.0).to_string());
                        if slider_response.hovered() {
                            self.visible_flag
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                        if slider_response.drag_stopped() {
                            info!("volumn slider dragged!");
                            audio_player.change_volumn(self.audio_volume);
                        }
                    });
                });
            }
        });
    }
    fn paint_fullscreen_button(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            let visible_num =
                f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
            let fullscreen_image_btn = egui::Button::new(
                Image::from(FULLSCREEN_IMG)
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
            let btn_response = ui.add(fullscreen_image_btn);
            if btn_response.hovered() {
                self.visible_flag
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if btn_response.clicked() {
                self.fullscreen_flag = !self.fullscreen_flag;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen_flag));
            }
        });
    }
}
