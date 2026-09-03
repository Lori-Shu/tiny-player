use std::sync::{Arc, atomic::AtomicU32};

use egui::{AtomExt, Color32, CornerRadius, Layout, RichText, Stroke, Ui, Vec2};
use egui_tiles::UiResponse;
use flume::Receiver;
use typed_builder::TypedBuilder;
#[derive(Debug, TypedBuilder)]
pub struct CaptionUI {
    visible_num: Arc<AtomicU32>,
    subtitle_text_receiver: Receiver<String>,
    subtitle_str: Option<String>,
    last_text_time: f64,
}
impl CaptionUI {
    fn paint_subtitle(&mut self, ui: &mut Ui) {
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
            if ui.time() - self.last_text_time > 2.0 {
                self.subtitle_str = None;
            }
        });
    }
    pub fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.paint_subtitle(ui);
        UiResponse::None
    }
}
