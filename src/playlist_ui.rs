use std::sync::{Arc, atomic::AtomicBool};

use egui::{AtomExt, Button, Image, Ui, Vec2, ViewportBuilder, ViewportId};
use egui_file::FileDialog;
use tokio::{runtime::Handle, sync::RwLock};
use tracing::info;

use crate::appui::{AppUI, ChangeInputContext, VideoDes};

pub struct PlayListUI {
    video_des: Arc<RwLock<Vec<VideoDes>>>,
    change_input_context: ChangeInputContext,
    live_mode: Arc<AtomicBool>,
    scan_folder_dialog: Arc<RwLock<FileDialog>>,
    runtime_handle: Handle,
    playlist_window_flag: Arc<AtomicBool>,
}
impl PlayListUI {
    pub fn new(
        change_input_context: ChangeInputContext,
        live_mode: Arc<AtomicBool>,
        runtime_handle: Handle,
        playlist_window_flag: Arc<AtomicBool>,
    ) -> Self {
        let video_des = Arc::new(RwLock::new(vec![]));
        let scan_folder_dialog = Arc::new(RwLock::new(FileDialog::select_folder()));
        Self {
            video_des,
            change_input_context,
            live_mode,
            scan_folder_dialog,
            runtime_handle,
            playlist_window_flag,
        }
    }
    pub fn show(&mut self, ui: &mut Ui) {
        let viewport_id = ViewportId::from_hash_of("playlist_ui");
        ui.send_viewport_cmd_to(
            viewport_id,
            egui::ViewportCommand::Title("playlist_ui".to_string()),
        );

        let viewport_builder = ViewportBuilder::default();
        let video_des = self.video_des.clone();
        let ctx = self.change_input_context.clone();
        let live_mode = self.live_mode.clone();
        let scan_folder_dialog = self.scan_folder_dialog.clone();
        let runtime_handle = self.runtime_handle.clone();
        let playlist_window_flag = self.playlist_window_flag.clone();
        ui.show_viewport_deferred(viewport_id, viewport_builder, move |ui, _class| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.vertical(|ui| {
                    let video_urls_scroll = egui::ScrollArea::vertical().max_height(500.0);

                    video_urls_scroll.show(ui, |ui| {
                        if let Ok(videos) = video_des.try_read() {
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
                                                let mut ctx = ctx.clone();
                                                ctx.path = des.path.clone();
                                                if AppUI::reset_media_input(ctx).is_ok() {
                                                    live_mode.store(
                                                        false,
                                                        std::sync::atomic::Ordering::Relaxed,
                                                    );
                                                    info!("change_format_input success");
                                                }
                                            }
                                        });
                                    } else {
                                        columns[1].vertical(|ui| {
                                            ui.add(image_btn);
                                            if ui.add(player_text_button).clicked() {
                                                let mut ctx = ctx.clone();
                                                ctx.path = des.path.clone();
                                                if AppUI::reset_media_input(ctx).is_ok() {
                                                    live_mode.store(
                                                        false,
                                                        std::sync::atomic::Ordering::Relaxed,
                                                    );
                                                    info!("change_format_input success");
                                                }
                                            }
                                        });
                                    }
                                }
                            });
                        }
                    });
                    if let Ok(mut dialog) = scan_folder_dialog.try_write() {
                        dialog.show(ui.ctx());
                        if ui.button("scan video folder").clicked() {
                            dialog.open();
                        }
                        if dialog.selected() {
                            if let Ok(mut videos) = video_des.try_write() {
                                videos.clear();
                            }

                            if let Some(path) = dialog.path() {
                                let video_des = video_des.clone();
                                let path = path.to_path_buf();
                                let ctx = ui.ctx().clone();
                                runtime_handle
                                    .spawn(AppUI::read_video_folder(ctx, path, video_des));
                            }
                        }
                    }
                });
            });
            ui.ctx().input(|state| {
                if state.viewport().close_requested() {
                    playlist_window_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
        });
    }
}
