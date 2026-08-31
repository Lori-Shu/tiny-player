//! The playlist_ui module manages the ui of a separate window
//! The ui is with respect to the content of media sources from local disks
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use egui::{
    AtomExt, Button, ColorImage, Image, ImageData, ImageSource, RichText, ScrollArea,
    TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder, ViewportId, WidgetText,
};
use egui_file_dialog::FileDialog;
use egui_tiles::{Behavior, UiResponse};
use ffmpeg_the_third::{format::stream::Disposition, media::Type};
use image::{EncodableLayout, RgbaImage};
use tokio::{runtime::Handle, sync::RwLock};
use tracing::{info, warn};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult,
    appui::{AppUI, ResetInputContext, VideoDes},
    resources::PLAY_IMG,
};

pub struct PlayListUI {
    local_medias_tree: Arc<RwLock<egui_tiles::Tree<PlayListUIPane>>>,
    play_list_tree_behavior: Arc<RwLock<PlayListTreeBehavior>>,
    playlist_window_flag: Arc<AtomicBool>,
}
impl PlayListUI {
    pub fn new(
        reset_input_context: ResetInputContext,
        live_mode: Arc<AtomicBool>,
        runtime_handle: Handle,
        playlist_window_flag: Arc<AtomicBool>,
    ) -> Self {
        let media_des_panes = Arc::new(RwLock::new(vec![]));
        let scan_folder_dialog = Arc::new(RwLock::new(FileDialog::new()));
        let mut tiles = egui_tiles::Tiles::default();
        let controlbar = PlayListControlbar::builder()
            .scan_folder_dialog(scan_folder_dialog)
            .runtime_handle(runtime_handle)
            .build();
        let controlbar_id = tiles.insert_new(egui_tiles::Tile::Pane(PlayListUIPane::Controlbar(
            Box::new(controlbar),
        )));
        let deses = DesList::builder()
            .live_mode(live_mode)
            .reset_input_ctx(reset_input_context)
            .build();
        let deslist_id = tiles.insert_new(egui_tiles::Tile::Pane(PlayListUIPane::DesList(
            Box::new(deses),
        )));
        let root = tiles.insert_vertical_tile(vec![controlbar_id, deslist_id]);
        let local_medias_tree = Arc::new(RwLock::new(egui_tiles::Tree::new(
            "local_medias_tree",
            root,
            tiles,
        )));
        let play_list_tree_behavior = Arc::new(RwLock::new(
            PlayListTreeBehavior::builder()
                .media_des_panes(media_des_panes)
                .build(),
        ));
        Self {
            local_medias_tree,
            play_list_tree_behavior,
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
        let playlist_window_flag = self.playlist_window_flag.clone();
        let local_medias_tree = self.local_medias_tree.clone();
        let play_list_tree_behavior = self.play_list_tree_behavior.clone();
        ui.show_viewport_deferred(viewport_id, viewport_builder, move |ui, _class| {
            egui::CentralPanel::default().show(ui, |ui| {
                if let Ok(mut local_medias_tree) = local_medias_tree.try_write()
                    && let Ok(mut play_list_tree_behavior) = play_list_tree_behavior.try_write()
                {
                    local_medias_tree.ui(&mut *play_list_tree_behavior, ui);
                }
            });
            ui.ctx().input(|state| {
                if state.viewport().close_requested() {
                    playlist_window_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
        });
    }
}
enum PlayListUIPane {
    Controlbar(Box<PlayListControlbar>),
    DesList(Box<DesList>),
}
#[derive(TypedBuilder)]
struct PlayListControlbar {
    scan_folder_dialog: Arc<RwLock<FileDialog>>,
    runtime_handle: Handle,
}
impl PlayListControlbar {
    fn ui(&self, ui: &mut Ui, video_des: Arc<RwLock<Vec<MediaDesPane>>>) -> UiResponse {
        if let Ok(mut dialog) = self.scan_folder_dialog.try_write() {
            dialog.update(ui.ctx());
            if ui
                .button(RichText::new("📂 scan media folder").size(32.0))
                .clicked()
            {
                dialog.pick_directory();
            }

            if let Some(path) = dialog.take_picked() {
                if let Ok(mut videos) = video_des.try_write() {
                    videos.clear();
                }
                let video_des = video_des.clone();
                let path = path.to_path_buf();

                self.runtime_handle.spawn(Self::scan_video_folder(
                    ui.ctx().clone(),
                    path,
                    video_des,
                ));
            }
        }
        UiResponse::None
    }
    async fn scan_video_folder(
        ctx: egui::Context,
        path: PathBuf,
        media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>,
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
                        video_targets.push(
                            MediaDesPane::builder()
                                .media_des(VideoDes {
                                    name: file_name.to_string(),
                                    path: en.path(),
                                    texture_handle,
                                })
                                .build(),
                        );
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
                if let Type::Video = stream.parameters().medium()
                    && let Disposition::ATTACHED_PIC = stream.disposition()
                {
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
        if let ImageSource::Bytes { bytes, .. } = PLAY_IMG
            && let Ok(dyn_img) = image::load_from_memory(bytes.as_bytes())
        {
            Ok(dyn_img.to_rgba8())
        } else {
            Err(anyhow::Error::msg("load PLAY_IMG failed"))
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
#[derive(TypedBuilder)]
struct DesList {
    reset_input_ctx: ResetInputContext,
    live_mode: Arc<AtomicBool>,
}
impl DesList {
    fn ui(&self, ui: &mut Ui, media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>) -> UiResponse {
        ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |ui| {
                if let Ok(media_des_panes) = media_des_panes.try_read() {
                    for item in media_des_panes.iter().enumerate() {
                        if item.0 % 2 == 0 {
                            let _ = item.1.ui(
                                &mut ui[0],
                                &self.reset_input_ctx,
                                self.live_mode.clone(),
                            );
                        } else {
                            let _ = item.1.ui(
                                &mut ui[1],
                                &self.reset_input_ctx,
                                self.live_mode.clone(),
                            );
                        }
                    }
                }
            });
        });
        UiResponse::None
    }
}
#[derive(TypedBuilder)]
struct MediaDesPane {
    media_des: VideoDes,
}
impl MediaDesPane {
    fn ui(&self, ui: &mut Ui, ctx: &ResetInputContext, live_mode: Arc<AtomicBool>) -> UiResponse {
        let image_btn = Button::new(
            Image::new(&self.media_des.texture_handle)
                .atom_size(Vec2::new(1920.0 / 6.0, 1080.0 / 6.0)),
        );

        let player_text_button = Button::new(self.media_des.name.clone());
        ui.add(image_btn);
        if ui.add(player_text_button).clicked() {
            let mut ctx = ctx.clone();
            ctx.path = self.media_des.path.clone();
            AppUI::reset_media_input(ctx);
            live_mode.store(false, std::sync::atomic::Ordering::Relaxed);
            info!("change_format_input success");
        }

        UiResponse::None
    }
}
#[derive(TypedBuilder)]
struct PlayListTreeBehavior {
    media_des_panes: Arc<RwLock<Vec<MediaDesPane>>>,
}

impl Behavior<PlayListUIPane> for PlayListTreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut PlayListUIPane,
    ) -> UiResponse {
        match pane {
            PlayListUIPane::Controlbar(s) => s.ui(ui, self.media_des_panes.clone()),
            PlayListUIPane::DesList(s) => s.ui(ui, self.media_des_panes.clone()),
        }
    }

    fn tab_title_for_pane(&mut self, _pane: &PlayListUIPane) -> egui::WidgetText {
        WidgetText::Text(String::new())
    }
}
