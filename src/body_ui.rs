use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32},
};

use egui::{Align2, AtomExt, Button, Color32, Id, Image, Pos2, Stroke, Ui, Vec2, epaint::Hsva};

use egui_tiles::UiResponse;
use flume::Receiver;
use media_engine::MediaEngine;
use tokio::{runtime::Handle, sync::Notify};
use typed_builder::TypedBuilder;

use crate::{
    audio_playback::AudioPlayer,
    resources::{PAUSE_IMG, PLAY_IMG},
};

#[derive(TypedBuilder)]
pub struct BodyUI {
    async_runtime: Handle,
    media_engine: Arc<MediaEngine>,
    media_source_flag: Arc<AtomicBool>,
    visible_num: Arc<AtomicU32>,
    pause_flag: Arc<AtomicBool>,
    audio_player: Arc<AudioPlayer>,
    play_tasks_notify: Arc<Notify>,
    transcribe_task_notify: Arc<Notify>,
    mel_bars_recv: Receiver<Vec<f32>>,
    bars_buffer: Vec<f32>,
}
impl BodyUI {
    fn paint_playpause_btn(&mut self, ui: &mut Ui) {
        if self
            .media_source_flag
            .load(std::sync::atomic::Ordering::Acquire)
        {
            egui::Area::new(Id::new("playpause button area"))
                .fixed_pos(Pos2::new(
                    ui.available_size()[0] / 2.0,
                    ui.content_rect().height() / 2.0,
                ))
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
    pub fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        // let test_btn=Button::new("test").fill(Color32::from_white_alpha(10)).min_size(Vec2::new(ui.available_width(), ui.available_height()));
        // let _=ui.add(test_btn);
        self.paint_playpause_btn(ui);
        self.paint_mel_bars(ui);
        UiResponse::None
    }
    fn paint_mel_bars(&mut self, ui: &mut Ui) {
        while let Ok(bars) = self.mel_bars_recv.try_recv() {
            self.bars_buffer = bars;
        }
        let media_source_info = self
            .async_runtime
            .block_on(self.media_engine.media_source_info());
        if let Ok(media_source_info) = media_source_info
            && media_source_info.stream_existence_flags.audio
            && !media_source_info.stream_existence_flags.video
        {
            let available_size = ui.available_size();
            ui.horizontal(|ui| {
                let mut max = 0.0;
                self.bars_buffer.iter().for_each(|i| {
                    if i.abs() > max {
                        max = i.abs();
                    }
                });
                for (idx, j) in self.bars_buffer.iter().enumerate() {
                    let portion = j.abs() / max;
                    let bar = Button::new("".atom_size(Vec2::new(
                        available_size[0] / 64.0,
                        portion * available_size[1],
                    )))
                    .fill(Hsva::new(
                        idx as f32 / self.bars_buffer.len() as f32,
                        0.75,
                        0.75,
                        0.75,
                    ));
                    ui.add(bar);
                }
            });
        }
    }
}
