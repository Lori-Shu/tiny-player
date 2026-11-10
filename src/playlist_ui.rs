//! The playlist_ui module manages the ui of a separate window
//! The ui is with respect to the content of media sources from local disks
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use egui::{
    AtomExt, Button, ColorImage, Image, ImageData, ImageSource, Response, RichText, ScrollArea,
    TextureHandle, TextureOptions, Ui, Vec2, Widget, WidgetText,
};
use egui_file_dialog::FileDialog;
use egui_tiles::{Behavior, UiResponse};
use ffmpeg_the_third::format::stream::Disposition;
use image::{EncodableLayout, RgbaImage};
use tokio::{runtime::Handle, sync::RwLock};
use tracing::{info, warn};

use crate::{PlayerResult, appui::MediaDes, resources::DEFAULT_COVER, state_reset::StateResetter};

pub struct PlaylistUI {
    local_medias_tree: egui_tiles::Tree<PlaylistUIPane>,
    play_list_tree_behavior: PlaylistTreeBehavior,
}
impl PlaylistUI {
    pub fn new(
        state_resetter: Arc<StateResetter>,
        live_mode: Arc<AtomicBool>,
        runtime_handle: Handle,
    ) -> Self {
        let media_des_panes = Arc::new(RwLock::new(vec![]));
        let scan_folder_dialog = Arc::new(RwLock::new(FileDialog::new()));
        let mut tiles = egui_tiles::Tiles::default();
        let controlbar = PlaylistControlbar::new(
            scan_folder_dialog.clone(),
            runtime_handle,
            media_des_panes.clone(),
            state_resetter.clone(),
            live_mode.clone(),
        );
        let controlbar_id = tiles.insert_new(egui_tiles::Tile::Pane(PlaylistUIPane::Controlbar(
            Box::new(controlbar),
        )));
        let deses = Deslist::new(media_des_panes.clone());
        let deslist_id = tiles.insert_new(egui_tiles::Tile::Pane(PlaylistUIPane::Deslist(
            Box::new(deses),
        )));
        let root = tiles.insert_vertical_tile(vec![controlbar_id, deslist_id]);
        let local_medias_tree =
            egui_tiles::Tree::new(egui::Id::unique("local_medias_tree"), root, tiles);
        let play_list_tree_behavior = PlaylistTreeBehavior::new();
        Self {
            local_medias_tree,
            play_list_tree_behavior,
        }
    }
}
impl Widget for &mut PlaylistUI {
    fn ui(self, ui: &mut Ui) -> Response {
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(
                15,
                23,
                42,
                (220.0) as u8,
            ))
            .corner_radius(egui::CornerRadius::same(8))
            .shadow(egui::Shadow {
                offset: [0, -4],
                blur: 16,
                spread: 0,
                color: egui::Color32::from_black_alpha(80),
            })
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                self.local_medias_tree
                    .ui(&mut self.play_list_tree_behavior, ui);
            });
        ui.response()
    }
}

enum PlaylistUIPane {
    Controlbar(Box<PlaylistControlbar>),
    Deslist(Box<Deslist>),
}

struct PlaylistControlbar {
    scan_folder_dialog: Arc<RwLock<FileDialog>>,
    runtime_handle: Handle,
    video_des: Arc<RwLock<Vec<MediaDesPane>>>,
    state_resetter: Arc<StateResetter>,
    live_mode: Arc<AtomicBool>,
}
impl PlaylistControlbar {
    pub fn new(
        scan_folder_dialog: Arc<RwLock<FileDialog>>,
        runtime_handle: Handle,
        video_des: Arc<RwLock<Vec<MediaDesPane>>>,
        state_resetter: Arc<StateResetter>,
        live_mode: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scan_folder_dialog,
            runtime_handle,
            video_des,
            state_resetter,
            live_mode,
        }
    }
}
impl PlaylistControlbar {
    async fn scan_video_folder(
        ctx: egui::Context,
        path: PathBuf,
        media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>,
        state_resetter: Arc<StateResetter>,
        live_mode: Arc<AtomicBool>,
    ) {
        let mut video_targets = media_des_panes.write().await;
        video_targets.clear();
        if let Ok(ite) = path.read_dir() {
            for entry in ite {
                if let Ok(en) = entry {
                    if let Ok(t) = en.file_type()
                        && let Some(file_name) = en.file_name().to_str()
                        && t.is_file()
                        && let Ok(cover) = Self::load_file_cover(&en.path()).await
                    {
                        let texture_handle =
                            Self::load_cover_texture(&ctx, &cover, file_name).await;
                        video_targets.push(MediaDesPane::new(
                            MediaDes {
                                name: file_name.to_string(),
                                path: en.path(),
                                texture_handle,
                            },
                            state_resetter.clone(),
                            live_mode.clone(),
                        ));
                    }
                } else {
                    warn!("read dir element err");
                }
            }
        }
    }
    async fn load_file_cover(file_path: &Path) -> PlayerResult<RgbaImage> {
        if let Ok(input) = &mut ffmpeg_the_third::format::input(file_path) {
            if input.duration() < 5_000_000 {
                return Err(anyhow::Error::msg("not a valid media file"));
            }
            let mut cover_idx = None;

            for stream in input.streams() {
                if Disposition::ATTACHED_PIC == stream.disposition() {
                    cover_idx = Some(stream.index());
                    break;
                }
            }
            if let Some(idx) = cover_idx {
                for packet in input.packets() {
                    if let Ok((stream, p)) = &packet
                        && stream.index() == idx
                        && let Some(cover_data) = p.data()
                        && let Ok(dyn_img) = image::load_from_memory(cover_data)
                    {
                        return Ok(dyn_img.to_rgba8());
                    }
                }
            }
        } else {
            return Err(anyhow::Error::msg("not a valid media file"));
        }
        if let ImageSource::Bytes { bytes, .. } = DEFAULT_COVER
            && let Ok(dyn_img) = image::load_from_memory(bytes.as_bytes())
        {
            Ok(dyn_img.to_rgba8())
        } else {
            warn!("load DEFAULT_COVER failed");
            Err(anyhow::Error::msg("load DEFAULT_COVER failed"))
        }
    }
    async fn load_cover_texture(
        ctx: &egui::Context,
        cover: &RgbaImage,
        name: &str,
    ) -> TextureHandle {
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
}
impl Widget for &mut PlaylistControlbar {
    fn ui(self, ui: &mut Ui) -> Response {
        if let Ok(mut dialog) = self.scan_folder_dialog.try_write() {
            dialog.update(ui);
            let folder_button = Button::new(RichText::new("📂").size(32.0));
            let btn_response = ui.add(folder_button);
            if btn_response.clicked() {
                dialog.pick_directory();
            }
            btn_response.on_hover_text("scan media folder");

            if let Some(path) = dialog.take_picked() {
                if let Ok(mut videos) = self.video_des.try_write() {
                    videos.clear();
                }
                let video_des = self.video_des.clone();
                let path = path.to_path_buf();

                self.runtime_handle
                    .spawn(PlaylistControlbar::scan_video_folder(
                        ui.ctx().clone(),
                        path,
                        video_des,
                        self.state_resetter.clone(),
                        self.live_mode.clone(),
                    ));
            }
        }
        ui.response()
    }
}

struct Deslist {
    media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>,
}
impl Deslist {
    fn new(media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>) -> Self {
        Self { media_des_panes }
    }
}
impl Widget for &mut Deslist {
    fn ui(self, ui: &mut Ui) -> Response {
        ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |ui| {
                if let Ok(media_des_panes) = self.media_des_panes.try_read() {
                    for item in media_des_panes.iter().enumerate() {
                        if item.0 % 2 == 0 {
                            ui[0].add(item.1);
                        } else {
                            ui[1].add(item.1);
                        }
                    }
                }
            });
        });
        ui.response()
    }
}

struct MediaDesPane {
    media_des: MediaDes,
    state_resetter: Arc<StateResetter>,
    live_mode: Arc<AtomicBool>,
}
impl MediaDesPane {
    fn new(
        media_des: MediaDes,
        state_resetter: Arc<StateResetter>,
        live_mode: Arc<AtomicBool>,
    ) -> Self {
        Self {
            media_des,
            state_resetter,
            live_mode,
        }
    }
}
impl Widget for &MediaDesPane {
    fn ui(self, ui: &mut Ui) -> Response {
        let available_width = ui.available_width();
        let image_btn = Button::new(
            Image::new(&self.media_des.texture_handle)
                .atom_size(Vec2::new(available_width / 1.25, 1080.0 / 6.0)),
        );

        let player_text_button = Button::new(&self.media_des.name);
        ui.add(image_btn);
        if ui.add(player_text_button).clicked() {
            self.state_resetter.reset_media_input(&self.media_des.path);
            self.live_mode
                .store(false, std::sync::atomic::Ordering::Relaxed);
            info!("change_format_input success");
        }

        ui.response()
    }
}

struct PlaylistTreeBehavior {}
impl PlaylistTreeBehavior {
    fn new() -> Self {
        Self {}
    }
}
impl Behavior<PlaylistUIPane> for PlaylistTreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut PlaylistUIPane,
    ) -> UiResponse {
        match pane {
            PlaylistUIPane::Controlbar(ctrlbar) => {
                ui.add(ctrlbar.as_mut());
            }
            PlaylistUIPane::Deslist(des_list) => {
                ui.add(des_list.as_mut());
            }
        };
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, _pane: &PlaylistUIPane) -> egui::WidgetText {
        WidgetText::Text(String::new())
    }
}
