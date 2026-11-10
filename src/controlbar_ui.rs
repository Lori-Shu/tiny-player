//! The controlbar_ui module manages a subarea of the user interface
//! The subarea includes a few control widgets (e.g., progress slider)
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64},
};

use egui::{AtomExt, Button, Color32, Image, Layout, Response, RichText, Ui, Vec2, Widget};
use time::{Time, format_description::FormatDescriptionV3};
use tokio::{runtime::Handle, sync::Notify};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    appui::AtomicF32,
    audio_playback::AudioPlayer,
    resources::{FULLSCREEN_IMG, SUBTITLE_IMG, VOLUME_IMG},
    whispercpp_transcriber::{AtomicTargetLanguage, TargetLanguage},
};
use media_engine::MediaEngine;
#[derive(TypedBuilder, Clone)]
pub struct ControlbarUI {
    current_main_stream_timestamp: Arc<AtomicI64>,
    media_source_flag: Arc<AtomicBool>,
    live_mode: Arc<AtomicBool>,
    time_text: String,
    audio_player: Arc<AudioPlayer>,
    media_engine: Arc<MediaEngine>,
    async_rt: Handle,
    show_subtitle_options_flag: bool,
    visible_num: Arc<AtomicF32>,
    audio_volume: f32,
    fullscreen_flag: bool,
    show_volume_slider_flag: bool,
    target_language: Arc<AtomicTargetLanguage>,
    transcribe_task_notify: Arc<Notify>,
    play_time: Time,
    time_formatter: FormatDescriptionV3<'static>,
}
impl ControlbarUI {
    pub fn paint_controlbar(&mut self, ui: &mut Ui) {
        if self
            .media_source_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let visible_num = self.visible_num.load();
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
                        self.paint_progress_slider(ui);
                        self.paint_caption_button(ui);
                        self.paint_volume_button(ui);
                        self.paint_fullscreen_button(ui);
                    });
                });
        }
    }
    fn paint_progress_slider(&mut self, ui: &mut Ui) {
        ui.scope(|ui| {
            let visible_num = self.visible_num.load();
            let slider_color =
                Color32::from_rgba_unmultiplied(6, 182, 212, (220.0 * visible_num) as u8);
            let (mut ts, end_ts) = if !self.live_mode.load(std::sync::atomic::Ordering::Relaxed) {
                let ts = self
                    .current_main_stream_timestamp
                    .load(std::sync::atomic::Ordering::Relaxed);
                let end_ts = {
                    if let Ok(media_source_info) = self.media_engine.try_borrow_media_source_info()
                    {
                        media_source_info
                            .as_ref()
                            .map(|media_source_info| media_source_info.end_timestamp)
                    } else {
                        None
                    }
                    .unwrap_or_default()
                };
                (ts, end_ts)
            } else {
                (0, 0)
            };
            // this is merely for the slider thumb to become transparent
            // ui.set_opacity(visible_num);
            let progress_slider = egui::Slider::new(&mut ts, 0..=end_ts)
                .show_value(false)
                .handle_shape(egui::style::HandleShape::Circle);
            {
                let available_width = ui.available_width();
                let slider_style = ui.style_mut();
                slider_style.spacing.slider_width = available_width / 2.0;
                slider_style.spacing.slider_rail_height = 4.0;
                slider_style.visuals.widgets.inactive.bg_fill = slider_color;
            }
            let slider_response = ui.add(progress_slider);
            let _ = ui.add(
                Button::new(
                    RichText::new(&self.time_text)
                        .size(20.0)
                        .color(slider_color),
                )
                .fill(Color32::from_white_alpha((32.0 * visible_num) as u8)),
            );
            if slider_response.drag_stopped() {
                info!("slider dragged!");
                let audio_player = self.audio_player.clone();
                let media_engine = self.media_engine.clone();
                self.async_rt.spawn(async move {
                    media_engine.seek_timestamp(ts).await;
                    audio_player.clear_source_queue();
                    audio_player.play();
                });
            }
        });
    }
    fn paint_caption_button(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            let visible_num = self.visible_num.load();
            let subtitle_btn = Button::new(
                Image::from(SUBTITLE_IMG)
                    .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                    .atom_size(Vec2::new(50.0, 50.0)),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                51,
                65,
                85,
                (220.0 * visible_num) as u8,
            ))
            .corner_radius(30);
            let btn_response = ui.add(subtitle_btn);
            if btn_response.clicked() {
                self.show_subtitle_options_flag = !self.show_subtitle_options_flag;
            }

            if self.show_subtitle_options_flag {
                let mut target = self.target_language.load();
                if ui
                    .radio_value(
                        &mut target,
                        TargetLanguage::None,
                        RichText::new("close").size(10.0).color(Color32::ORANGE),
                    )
                    .clicked()
                {
                    self.target_language.store(target.clone());
                }
                if ui
                    .radio_value(
                        &mut target,
                        TargetLanguage::Chinese,
                        RichText::new("中文").size(10.0).color(Color32::ORANGE),
                    )
                    .clicked()
                {
                    self.target_language.store(target.clone());
                    self.transcribe_task_notify.notify_one();
                }
                if ui
                    .radio_value(
                        &mut target,
                        TargetLanguage::English,
                        RichText::new("English").size(10.0).color(Color32::ORANGE),
                    )
                    .clicked()
                {
                    self.target_language.store(target.clone());
                    self.transcribe_task_notify.notify_one();
                }
            }
        });
    }
    fn paint_volume_button(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            let visible_num = self.visible_num.load();
            let volumn_img_btn = egui::Button::new(
                Image::from(VOLUME_IMG)
                    .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                    .atom_size(Vec2::new(50.0, 50.0)),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                51,
                65,
                85,
                (220.0 * visible_num) as u8,
            ))
            .corner_radius(30);
            let btn_response = ui.add(volumn_img_btn);
            if btn_response.clicked() {
                self.show_volume_slider_flag = !self.show_volume_slider_flag;
            }
            if self.show_volume_slider_flag {
                let visible_num = self.visible_num.load();
                let audio_player = &mut self.audio_player;
                egui::Area::new(egui::Id::unique("volume_slider_popup"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(btn_response.rect.left_top() - egui::vec2(-32.0, 180.0))
                    .show(ui.ctx(), |ui| {
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
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                let slider_color = Color32::from_rgba_unmultiplied(
                                    6,
                                    182,
                                    212,
                                    (220.0 * visible_num) as u8,
                                );
                                // this is merely for the slider thumb to become transparent
                                // ui.set_opacity(visible_num);
                                let volume_slider =
                                    egui::Slider::new(&mut self.audio_volume, 0.0..=2.0)
                                        .vertical()
                                        .show_value(false)
                                        .handle_shape(egui::style::HandleShape::Circle);
                                {
                                    let slider_style = ui.style_mut();
                                    slider_style.spacing.slider_width = 150.0;
                                    slider_style.spacing.slider_rail_height = 4.0;
                                    slider_style.visuals.widgets.inactive.bg_fill = slider_color;
                                }
                                let mut slider_response = ui.add(volume_slider);
                                slider_response = slider_response
                                    .on_hover_text((self.audio_volume * 100.0).to_string());
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
            let visible_num = self.visible_num.load();
            let fullscreen_image_btn = egui::Button::new(
                Image::from(FULLSCREEN_IMG)
                    .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                    .atom_size(Vec2::new(50.0, 50.0)),
            )
            .fill(egui::Color32::from_rgba_unmultiplied(
                51,
                65,
                85,
                (220.0 * visible_num) as u8,
            ))
            .corner_radius(30);
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
        let sec_num = {
            if let Ok(media_source_info) = self.media_engine.try_borrow_media_source_info()
                && let Some(media_source_info) = &*media_source_info
                && self
                    .media_source_flag
                    .load(std::sync::atomic::Ordering::Relaxed)
                && !self.live_mode.load(std::sync::atomic::Ordering::Relaxed)
            {
                let play_ts = self
                    .current_main_stream_timestamp
                    .load(std::sync::atomic::Ordering::Relaxed);
                {
                    if media_source_info.stream_existence_flags.audio {
                        let audio_time_base = media_source_info.audio_time_base;
                        play_ts * audio_time_base.numerator() as i64
                            / audio_time_base.denominator() as i64
                    } else {
                        let v_time_base = media_source_info.video_time_base;

                        play_ts * v_time_base.numerator() as i64 / v_time_base.denominator() as i64
                    }
                }
            } else {
                0
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
    fn update_time_text(&mut self) {
        if let Ok(mut now_str) = self.play_time.format(&self.time_formatter) {
            {
                if let Ok(media_source_info) = self.media_engine.try_borrow_media_source_info()
                    && let Some(info) = &*media_source_info
                {
                    now_str.push('|');
                    now_str.push_str(&info.end_time_formatted_str);
                }
            }
            self.set_time_text(now_str);
        }
    }
}
impl Widget for &mut ControlbarUI {
    fn ui(self, ui: &mut Ui) -> Response {
        self.update_time();
        self.update_time_text();
        ui.with_layout(Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            self.paint_controlbar(ui);
        });
        ui.response()
    }
}
