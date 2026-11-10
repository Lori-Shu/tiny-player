use std::sync::{Arc, atomic::AtomicBool};

use egui::{Align2, AtomExt, Color32, Id, Image, Pos2, Response, Ui, Vec2, Widget, epaint::Hsva};

use egui_plot::{Bar, BarChart, Plot};
use flume::Receiver;
use media_engine::MediaEngine;
use tokio::sync::Notify;
use typed_builder::TypedBuilder;

use crate::{
    appui::AtomicF32,
    audio_playback::AudioPlayer,
    resources::{PAUSE_IMG, PLAY_IMG},
};

#[derive(TypedBuilder)]
pub struct BodyUI {
    media_engine: Arc<MediaEngine>,
    media_source_flag: Arc<AtomicBool>,
    visible_num: Arc<AtomicF32>,
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
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            egui::Area::new(Id::unique("playpause button area"))
                .fixed_pos(Pos2::new(
                    ui.available_size()[0] / 2.0,
                    ui.content_rect().height() / 2.0,
                ))
                .pivot(Align2::CENTER_CENTER)
                .show(ui, |ui| {
                    let visible_num = self.visible_num.load();
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
                        .fill(Color32::from_rgba_unmultiplied(
                            51,
                            65,
                            85,
                            (220.0 * visible_num) as u8,
                        ))
                        .corner_radius(30);
                    let btn_response = ui.add(play_or_pause_btn);
                    if btn_response.clicked()
                        || ui.ctx().input(|s| s.key_released(egui::Key::Space))
                    {
                        let pause_flag = &self.pause_flag;
                        let previous_v = pause_flag.load(std::sync::atomic::Ordering::Relaxed);
                        pause_flag.store(!previous_v, std::sync::atomic::Ordering::Relaxed);
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

    fn paint_mel_bars(&mut self, ui: &mut Ui) {
        while let Ok(bars) = self.mel_bars_recv.try_recv() {
            self.bars_buffer = bars;
        }
        if let Ok(media_source_info) = self.media_engine.try_borrow_media_source_info()
            && let Some(media_source_info) = &*media_source_info
            && media_source_info.stream_existence_flags.audio
            && !media_source_info.stream_existence_flags.video
        {
            let mut max = 0.0;
            let display_nums = self
                .bars_buffer
                .iter()
                .filter_map(|i| {
                    if i.abs() > 75.0 {
                        return None;
                    }
                    if i.abs() > max {
                        max = i.abs();
                    }
                    Some(i.abs())
                })
                .collect::<Vec<f32>>();
            let display_bars = display_nums
                .iter()
                .enumerate()
                .map(|(idx, n)| {
                    let portion = n / max;
                    Bar::new(
                        idx as f64,
                        (portion * ui.content_rect().height() / 4.0) as f64,
                    )
                    .fill(Hsva::new(
                        idx as f32 / display_nums.len() as f32,
                        0.75,
                        0.75,
                        0.75,
                    ))
                })
                .collect();
            let bar_chart = BarChart::new("human feeling bars", display_bars);
            let plot = Plot::new("barchart plot")
                .show_background(false)
                .allow_axis_zoom_drag(false)
                .allow_drag(false)
                .allow_scroll(false)
                .allow_zoom(false);
            plot.show(ui, |ui| {
                ui.add(bar_chart);
            });
        }
    }
}
impl Widget for &mut BodyUI {
    fn ui(self, ui: &mut Ui) -> Response {
        // let test_btn=Button::new("test").fill(Color32::from_white_alpha(10)).min_size(Vec2::new(ui.available_width(), ui.available_height()));
        // let _=ui.add(test_btn);
        self.paint_playpause_btn(ui);
        self.paint_mel_bars(ui);
        ui.response()
    }
}
