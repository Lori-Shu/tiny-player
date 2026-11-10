use std::sync::{Arc, atomic::AtomicBool};

use egui::{Response, RichText, Ui, Widget};
use tokio::sync::RwLock;
use tracing::warn;
pub enum TipMessageType {
    Warn(String),
    Info(String),
}
pub struct TipMessageModal {
    tip_modal_flag: Arc<AtomicBool>,
    tip_modal_msg: Arc<RwLock<TipMessageType>>,
}
impl TipMessageModal {
    pub fn new(
        tip_modal_flag: Arc<AtomicBool>,
        tip_modal_msg: Arc<RwLock<TipMessageType>>,
    ) -> Self {
        Self {
            tip_modal_flag,
            tip_modal_msg,
        }
    }
}
impl Widget for &TipMessageModal {
    fn ui(self, ui: &mut Ui) -> Response {
        if self
            .tip_modal_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let tip_modal = egui::Modal::new(egui::Id::unique("tip message modal"));
            tip_modal.show(ui, |ui| {
                let tip_text = if let Ok(tip_modal_msg) = self.tip_modal_msg.try_read() {
                    let display_msg = match &*tip_modal_msg {
                        TipMessageType::Info(tip_modal_msg) => {
                            let mut emoji_str = String::from("ℹ️");
                            emoji_str.push_str(tip_modal_msg);
                            emoji_str
                        }
                        TipMessageType::Warn(tip_modal_msg) => {
                            let mut emoji_str = String::from("❌");
                            emoji_str.push_str(tip_modal_msg);
                            emoji_str
                        }
                    };
                    RichText::new(display_msg).size(20.0)
                } else {
                    warn!("try read err msg failed");
                    return;
                };
                ui.add(egui::Label::new(tip_text));
                if ui.button("close").clicked() {
                    self.tip_modal_flag
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
        ui.response()
    }
}
