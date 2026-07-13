//! The controlbar_ui module manages a subarea of the user interface
//! The subarea includes a few control widgets (e.g., progress slider)
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicU32},
};

use egui::{AtomExt, Button, Color32, Image, Layout, RichText, Stroke, Ui, Vec2};
use egui_tiles::UiResponse;
use time::{Time, format_description::OwnedFormatItem};
use tokio::{
    runtime::Handle,
    sync::{Notify, RwLock},
};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    audio_playback::AudioPlayer,
    decode_engine::{MainStream, TinyDecoder},
    resources::{FULLSCREEN_IMG, SUBTITLE_IMG, VOLUME_IMG},
    whispercpp_transcriber::UsedModel,
};
#[derive(TypedBuilder, Clone)]
pub struct ControlbarUI {
    current_main_stream_timestamp: Arc<AtomicI64>,
    media_source_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
    end_ts: Arc<AtomicI64>,
    time_text: String,
    audio_player: Arc<AudioPlayer>,
    tiny_decoder: Arc<RwLock<TinyDecoder>>,
    async_rt: Handle,
    show_subtitle_options_flag: bool,
    visible_num: Arc<AtomicU32>,
    audio_volume: f32,
    fullscreen_flag: bool,
    show_volume_slider_flag: bool,
    used_model: Arc<RwLock<UsedModel>>,
    transcribe_task_notify: Arc<Notify>,
    play_time: Time,
    time_formatter: OwnedFormatItem,
}
impl ControlbarUI {
    pub fn paint_controlbar(&mut self, ui: &mut Ui) {
        let visible_num =
            f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(
                15,
                23,
                42,
                (220.0 * visible_num) as u8,
            ))
            .corner_radius(egui::CornerRadius::same(8))
            .shadow(egui::Shadow {
                offset: [0, -4],
                blur: 16,
                spread: 0,
                color: egui::Color32::from_black_alpha((80.0 * visible_num) as u8),
            })
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
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
            });
    }
    fn paint_progress_slider(&mut self, ui: &mut Ui) {
        let mut slider_color = Color32::ORANGE.to_srgba_unmultiplied();
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
            let progress_slider = egui::Slider::new(&mut ts, 0..=end_ts).show_value(false);

            let mut slider_width_style = egui::style::Style::default();
            slider_width_style.spacing.slider_width = ui.content_rect().width() / 2.0;
            slider_width_style.spacing.slider_rail_height = 4.0;
            slider_width_style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
            slider_width_style.spacing.interact_size = Vec2::new(20.0, 20.0);
            slider_width_style.visuals.widgets.inactive.bg_fill =
                Color32::from_rgba_unmultiplied(255, 165, 0, 200);
            slider_width_style.spacing.item_spacing.x = 12.0;
            ui.set_style(slider_width_style);
            let slider_response = ui.add(progress_slider);
            let _ = ui.add(
                Button::new(RichText::new(self.time_text.clone()).size(20.0).color(
                    Color32::from_rgba_unmultiplied(
                        slider_color[0],
                        slider_color[1],
                        slider_color[2],
                        slider_color[3],
                    ),
                ))
                .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8)),
            );
            if slider_response.drag_stopped() {
                info!("slider dragged!");
                let audio_player = self.audio_player.clone();
                let tiny_decoder = self.tiny_decoder.clone();
                self.async_rt.spawn(async move {
                    let tiny_decoder = tiny_decoder.read().await;
                    tiny_decoder.seek_timestamp_to_decode(ts).await;
                    audio_player.clear_source_queue();
                    audio_player.play();
                });
            }
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
            .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
            .stroke(Stroke::new(
                1.0,
                Color32::from_white_alpha((10.0 * visible_num) as u8),
            ));
            let btn_response = ui.add(subtitle_btn);
            if btn_response.clicked() {
                self.show_subtitle_options_flag = !self.show_subtitle_options_flag;
            }

            if self.show_subtitle_options_flag
                && let Ok(mut used_model) = self.used_model.try_write()
            {
                ui.radio_value(
                    &mut *used_model,
                    UsedModel::None,
                    RichText::new("close").size(10.0).color(Color32::ORANGE),
                );
                if ui
                    .radio_value(
                        &mut *used_model,
                        UsedModel::Chinese,
                        RichText::new("中文").size(10.0).color(Color32::ORANGE),
                    )
                    .clicked()
                {
                    self.transcribe_task_notify.notify_one();
                }
                if ui
                    .radio_value(
                        &mut *used_model,
                        UsedModel::English,
                        RichText::new("English").size(10.0).color(Color32::ORANGE),
                    )
                    .clicked()
                {
                    self.transcribe_task_notify.notify_one();
                }
            }
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
            .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
            .stroke(Stroke::new(
                1.0,
                Color32::from_white_alpha((10.0 * visible_num) as u8),
            ));
            let btn_response = ui.add(volumn_img_btn);
            if btn_response.clicked() {
                self.show_volume_slider_flag = !self.show_volume_slider_flag;
            }
            if self.show_volume_slider_flag {
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    let visible_num =
                        f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    let audio_player = &mut self.audio_player;
                    ui.scope(|ui| {
                        ui.set_opacity(visible_num);
                        let volume_slider = egui::Slider::new(&mut self.audio_volume, 0.0..=2.0)
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

                        let mut slider_response =
                            ui.add_sized(Vec2::new(10.0, 150.0), volume_slider);
                        slider_response =
                            slider_response.on_hover_text((self.audio_volume * 100.0).to_string());
                        if slider_response.drag_stopped() {
                            info!("volumn slider dragged!");
                            audio_player.adjust_volume(self.audio_volume);
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
            .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
            .stroke(Stroke::new(
                1.0,
                Color32::from_white_alpha((10.0 * visible_num) as u8),
            ));
            let btn_response = ui.add(fullscreen_image_btn);
            if btn_response.clicked() {
                self.fullscreen_flag = !self.fullscreen_flag;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen_flag));
            }
        });
    }
    pub fn set_time_text(&mut self, s: String) {
        self.time_text = s;
    }
    fn update_time(&mut self) {
        if let Ok(tiny_decoder) = self.tiny_decoder.try_read()
            && self
                .media_source_flag
                .load(std::sync::atomic::Ordering::Acquire)
            && !self.live_mode.load(std::sync::atomic::Ordering::Relaxed)
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
        if let Ok(mut now_str) = self.play_time.format(&self.time_formatter) {
            if let Ok(tiny_decoder) = self.tiny_decoder.try_read() {
                now_str.push('|');
                now_str.push_str(&tiny_decoder.end_time_formatted_string);
            }
            self.set_time_text(now_str);
        }
    }
    pub fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.update_time();
        self.update_time_text();
        ui.with_layout(Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            self.paint_controlbar(ui);
        });
        UiResponse::None
    }
}
