//! The internet_resource_ui module manages the ui of a separate window
//! The ui is with respect to the content of online resources
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use egui::{
    Button, CentralPanel, RadioButton, RichText, ScrollArea, Ui, ViewportBuilder, ViewportId,
};
use egui_tiles::{Behavior, TileId, UiResponse};
use futures_util::StreamExt;
use quick_m3u8::config::ParsingOptions;
use reqwest::Client;
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::RwLock,
};
use typed_builder::TypedBuilder;

use crate::{
    PlayerResult,
    appui::{AppUI, ResetInputContext},
};
const ENGLISH_PLAYLIST_URL: &str = "https://iptv-org.github.io/iptv/languages/eng.m3u";
const CHINESE_PLAYLIST_URL: &str = "https://iptv-org.github.io/iptv/languages/zho.m3u";
#[derive(Debug, Clone)]
pub struct MediaResource {
    pub name: String,
}
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum LanguageCategory {
    None,
    Chinese,
    English,
}

pub struct InternetResourceUI {
    ui_tree: Arc<RwLock<egui_tiles::Tree<InternetResourceUIPane>>>,
    tree_behavior: Arc<RwLock<InternetResourceUIPaneBehavior>>,
}

impl InternetResourceUI {
    pub fn new(
        change_input_ctx: ResetInputContext,
        internet_list_window_flag: Arc<AtomicBool>,
    ) -> Self {
        let change_input_ctx = Arc::new(RwLock::new(change_input_ctx));
        let mut tiles = egui_tiles::Tiles::default();
        let resources = Arc::new(RwLock::new(vec![]));
        let selectable_area = SelectableArea::new();
        let scrollable_area = ScrollableArea::new(change_input_ctx.clone());
        let selectable_id = tiles.insert_new(egui_tiles::Tile::Pane(
            InternetResourceUIPane::Selectable(Box::new(selectable_area)),
        ));
        let scrollable_id = tiles.insert_new(egui_tiles::Tile::Pane(
            InternetResourceUIPane::Scrollable(Box::new(scrollable_area)),
        ));
        let root = tiles.insert_vertical_tile(vec![selectable_id, scrollable_id]);
        let ui_tree = Arc::new(RwLock::new(egui_tiles::Tree::new(
            "internet_resource_ui_tree",
            root,
            tiles,
        )));
        let tree_behavior = Arc::new(RwLock::new(
            InternetResourceUIPaneBehavior::builder()
                .change_input_ctx(change_input_ctx)
                .internet_list_window_flag(internet_list_window_flag)
                .resources(resources)
                .build(),
        ));

        Self {
            ui_tree,
            tree_behavior,
        }
    }
    pub fn show(&mut self, ui: &mut Ui) {
        let viewport_id = ViewportId::from_hash_of("internet_resource_ui");
        ui.send_viewport_cmd_to(
            viewport_id,
            egui::ViewportCommand::Title("internet_resource_ui".to_string()),
        );
        let ui_tree = self.ui_tree.clone();
        let tree_behavior = self.tree_behavior.clone();
        ui.show_viewport_deferred(
            viewport_id,
            ViewportBuilder::default(),
            move |ui, _viewport_class| {
                CentralPanel::default().show(ui, |ui| {
                    if let Ok(mut ui_tree) = ui_tree.try_write()
                        && let Ok(mut tree_behavior) = tree_behavior.try_write()
                    {
                        ui_tree.ui(&mut *tree_behavior, ui);
                    }
                });
            },
        );
    }
}
enum InternetResourceUIPane {
    Selectable(Box<SelectableArea>),
    Scrollable(Box<ScrollableArea>),
}
#[derive(TypedBuilder)]
struct InternetResourceUIPaneBehavior {
    change_input_ctx: Arc<RwLock<ResetInputContext>>,
    internet_list_window_flag: Arc<AtomicBool>,
    resources: Arc<RwLock<Vec<SingleResourcePane>>>,
}

impl Behavior<InternetResourceUIPane> for InternetResourceUIPaneBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        pane: &mut InternetResourceUIPane,
    ) -> UiResponse {
        match pane {
            InternetResourceUIPane::Selectable(s) => s.ui(
                ui,
                self.resources.clone(),
                self.internet_list_window_flag.clone(),
                self.change_input_ctx.clone(),
            ),
            InternetResourceUIPane::Scrollable(s) => s.ui(ui, self.resources.clone()),
        }
    }

    fn tab_title_for_pane(&mut self, _pane: &InternetResourceUIPane) -> egui::WidgetText {
        egui::WidgetText::Text(String::new())
    }
}

#[derive(Debug)]
struct SelectableArea {
    language_category: LanguageCategory,
    clicked: bool,
    web_client: Client,
}
impl SelectableArea {
    fn new() -> Self {
        let language_category = LanguageCategory::None;
        let clicked = false;
        let web_client = Client::new();
        Self {
            language_category,
            clicked,
            web_client,
        }
    }
    fn ui(
        &mut self,
        ui: &mut Ui,
        resources: Arc<RwLock<Vec<SingleResourcePane>>>,
        internet_list_window_flag: Arc<AtomicBool>,
        change_input_ctx: Arc<RwLock<ResetInputContext>>,
    ) -> UiResponse {
        ui.ctx().input(|state| {
            if state.viewport().close_requested() {
                internet_list_window_flag.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add(RadioButton::new(
                    self.language_category == LanguageCategory::None,
                    RichText::new("None⭕").size(32.0),
                ))
                .clicked()
            {
                self.language_category = LanguageCategory::None;
                if let Ok(mut resources) = resources.try_write() {
                    resources.clear();
                }
            }
            if ui
                .add(RadioButton::new(
                    self.language_category == LanguageCategory::English,
                    RichText::new("English🔤").size(32.0),
                ))
                .clicked()
            {
                self.language_category = LanguageCategory::English;
                self.clicked = true;
            }
            if ui
                .add(RadioButton::new(
                    self.language_category == LanguageCategory::Chinese,
                    RichText::new("Chinese🉐").size(32.0),
                ))
                .clicked()
            {
                self.language_category = LanguageCategory::Chinese;
                self.clicked = true;
            }
        });
        if self.clicked
            && let Ok(change_input_ctx) = change_input_ctx.try_read()
        {
            change_input_ctx
                .runtime_handle
                .spawn(Self::request_playlist(
                    self.language_category.clone(),
                    self.web_client.clone(),
                    resources.clone(),
                ));
            self.clicked = false;
        }
        ui.separator();
        UiResponse::None
    }
    async fn request_playlist(
        current_category: LanguageCategory,
        web_client: Client,
        resources: Arc<RwLock<Vec<SingleResourcePane>>>,
    ) -> PlayerResult<()> {
        let url = match &current_category {
            LanguageCategory::English => ENGLISH_PLAYLIST_URL,
            LanguageCategory::Chinese => CHINESE_PLAYLIST_URL,
            LanguageCategory::None => {
                return Err(anyhow::Error::msg("None Category selected"));
            }
        };

        let response = web_client.get(url).send().await?;
        let bytes_stream = response
            .bytes_stream()
            .map(|item| item.map_err(std::io::Error::other));

        let mut buf_reader = BufReader::new(tokio_util::io::StreamReader::new(bytes_stream));
        let mut buf = vec![0; 1024 * 32];
        let read_size = buf_reader.read(&mut buf).await?;
        let mut reader =
            quick_m3u8::Reader::from_bytes(&buf[0..read_size], ParsingOptions::default());
        let mut resources = resources.write().await;
        resources.clear();
        while let Ok(Some(hls_line)) = reader.read_line() {
            if let quick_m3u8::HlsLine::Uri(uri) = hls_line {
                let single_resource_pane = SingleResourcePane::new(MediaResource {
                    name: uri.to_string(),
                });
                resources.push(single_resource_pane);
            }
        }

        Ok(())
    }
}

struct ScrollableArea {
    change_input_ctx: Arc<RwLock<ResetInputContext>>,
}
impl ScrollableArea {
    fn new(change_input_ctx: Arc<RwLock<ResetInputContext>>) -> Self {
        Self { change_input_ctx }
    }
    fn ui(&self, ui: &mut Ui, resources: Arc<RwLock<Vec<SingleResourcePane>>>) -> UiResponse {
        ScrollArea::vertical().show(ui, |ui| {
            if let Ok(resources) = resources.try_read() {
                for resource_pane in &*resources {
                    let _ = resource_pane.ui(ui, self.change_input_ctx.clone());
                }
            }
        });
        UiResponse::None
    }
}
#[derive(Debug)]
struct SingleResourcePane {
    resource: MediaResource,
}
impl SingleResourcePane {
    fn new(resource: MediaResource) -> Self {
        Self { resource }
    }
    fn ui(&self, ui: &mut Ui, change_input_ctx: Arc<RwLock<ResetInputContext>>) -> UiResponse {
        ui.set_min_height(100.0);
        let btn_response = ui.add(Button::new(&self.resource.name));
        if btn_response.clicked()
            && let Ok(mut context) = change_input_ctx.try_write()
        {
            context.path = PathBuf::from(&self.resource.name);

            AppUI::reset_media_input(context.clone());

            context
                .live_mode
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        UiResponse::None
    }
}
