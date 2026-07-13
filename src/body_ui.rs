use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32},
};

use egui::{Align2, AtomExt, Color32, CornerRadius, Id, Image, Layout, RichText, Stroke, Ui, Vec2};
use egui_tiles::UiResponse;
use flume::Receiver;
use tokio::sync::Notify;
use typed_builder::TypedBuilder;

use crate::{
    audio_playback::AudioPlayer,
    resources::{PAUSE_IMG, PLAY_IMG},
};

#[derive(TypedBuilder)]
pub struct BodyUI {
    media_source_flag: Arc<AtomicBool>,
    visible_num: Arc<AtomicU32>,
    pause_flag: Arc<AtomicBool>,
    audio_player: Arc<AudioPlayer>,
    play_tasks_notify: Arc<Notify>,
    transcribe_task_notify: Arc<Notify>,
    subtitle_text_receiver: Receiver<String>,
    subtitle_str: Option<String>,
    last_text_time: f64,
}
impl BodyUI {
    fn paint_playpause_btn(&mut self, ui: &mut Ui) {
        if self
            .media_source_flag
            .load(std::sync::atomic::Ordering::Acquire)
        {
            egui::Area::new(Id::new("playpause button area"))
                .fixed_pos(ui.content_rect().center())
                .pivot(Align2::CENTER_CENTER)
                .show(ui.ctx(), |ui| {
                    let visible_num =
                        f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    let play_or_pause_image_source =
                        if self.pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
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
                        .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
                        .stroke(Stroke::new(
                            1.0,
                            Color32::from_white_alpha((10.0 * visible_num) as u8),
                        ));

                    let btn_response = ui.add(play_or_pause_btn);
                    if btn_response.clicked()
                        || ui.ctx().input(|s| s.key_released(egui::Key::Space))
                    {
                        let pause_flag = &self.pause_flag;
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
                .media_source_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                    if let Ok(generated_str) = self.subtitle_text_receiver.try_recv() {
                        self.subtitle_str = Some(generated_str);
                        self.last_text_time = ui.time();
                    }
                    let visible_num =
                        f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
                    if let Some(subtitle_str) = &self.subtitle_str {
                        let subtitle_text_button = egui::Button::new(
                            RichText::new(subtitle_str)
                                .size(35.0)
                                .color(Color32::ORANGE)
                                .atom_size(Vec2::new(ui.content_rect().width(), 35.0)),
                        )
                        .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
                        .stroke(Stroke::new(
                            1.0,
                            Color32::from_black_alpha((10.0 * visible_num) as u8),
                        ))
                        .corner_radius(CornerRadius::from(30));
                        ui.add(subtitle_text_button);
                    }
                });
            }
            if ui.time() - self.last_text_time > 2.0 {
                self.subtitle_str = None;
            }
        });
    }
    pub fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        // let test_btn=Button::new("test").fill(Color32::from_white_alpha(10)).min_size(Vec2::new(ui.available_width(), ui.available_height()));
        // let _=ui.add(test_btn);
        self.paint_playpause_btn(ui);
        ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
            self.paint_subtitle(ui);
        });
        UiResponse::None
    }
}
