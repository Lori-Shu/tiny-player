//! The internet_resource_ui module manages the ui of a separate window
//! The ui is with respect to the content of online resources
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use egui::{Button, CentralPanel, RadioButton, Response, RichText, ScrollArea, Ui, Widget};
use egui_tiles::{Behavior, TileId, Tree, UiResponse};
use futures_util::StreamExt;
use quick_m3u8::config::ParsingOptions;
use reqwest::Client;
use tokio::{
    io::{AsyncReadExt, BufReader},
    runtime::Handle,
    sync::RwLock,
};
use tracing::{info, warn};

use crate::state_reset::StateResetter;
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
    ui_tree: Tree<InternetResourceUIPane>,
    tree_behavior: InternetResourceUIPaneBehavior,
}

impl InternetResourceUI {
    pub fn new(
        state_resetter: Arc<StateResetter>,
        internet_list_window_flag: Arc<AtomicBool>,
        handle: Handle,
        live_mode: Arc<AtomicBool>,
    ) -> Self {
        let mut tiles = egui_tiles::Tiles::default();
        let resources = Arc::new(RwLock::new(vec![]));
        let selectable_area = SelectableArea::new(
            live_mode.clone(),
            handle,
            internet_list_window_flag,
            resources.clone(),
            state_resetter.clone(),
        );
        let scrollable_area = ScrollableArea::new(resources.clone());
        let selectable_id = tiles.insert_new(egui_tiles::Tile::Pane(
            InternetResourceUIPane::Selectable(Box::new(selectable_area)),
        ));
        let scrollable_id = tiles.insert_new(egui_tiles::Tile::Pane(
            InternetResourceUIPane::Scrollable(Box::new(scrollable_area)),
        ));
        let root = tiles.insert_vertical_tile(vec![selectable_id, scrollable_id]);
        let ui_tree = Tree::new(egui::Id::unique("internet_resource_ui_tree"), root, tiles);
        let tree_behavior = InternetResourceUIPaneBehavior::new();

        Self {
            ui_tree,
            tree_behavior,
        }
    }
}
impl Widget for &mut InternetResourceUI {
    fn ui(self, ui: &mut Ui) -> Response {
        let window = egui::Window::new("internet_resource_ui");
        window.show(ui, |ui| {
            CentralPanel::default().show(ui, |ui| {
                self.ui_tree.ui(&mut self.tree_behavior, ui);
            });
        });

        ui.response()
    }
}
enum InternetResourceUIPane {
    Selectable(Box<SelectableArea>),
    Scrollable(Box<ScrollableArea>),
}

struct InternetResourceUIPaneBehavior {}
impl InternetResourceUIPaneBehavior {
    pub fn new() -> Self {
        Self {}
    }
}

impl Behavior<InternetResourceUIPane> for InternetResourceUIPaneBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        pane: &mut InternetResourceUIPane,
    ) -> UiResponse {
        match pane {
            InternetResourceUIPane::Selectable(selectable_area) => {
                ui.add(selectable_area.as_mut());
            }
            InternetResourceUIPane::Scrollable(scrollable_area) => {
                ui.add(scrollable_area.as_mut());
            }
        };
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, _pane: &InternetResourceUIPane) -> egui::WidgetText {
        egui::WidgetText::Text(String::new())
    }
}

struct SelectableArea {
    language_category: LanguageCategory,
    clicked: bool,
    web_client: Client,
    live_mode: Arc<AtomicBool>,
    handle: Handle,
    internet_list_window_flag: Arc<AtomicBool>,
    resources: Arc<RwLock<Vec<SingleResourcePane>>>,
    state_resetter: Arc<StateResetter>,
}
impl SelectableArea {
    fn new(
        live_mode: Arc<AtomicBool>,
        handle: Handle,
        internet_list_window_flag: Arc<AtomicBool>,
        resources: Arc<RwLock<Vec<SingleResourcePane>>>,
        state_resetter: Arc<StateResetter>,
    ) -> Self {
        let language_category = LanguageCategory::None;
        let clicked = false;
        let web_client = Client::new();
        Self {
            language_category,
            clicked,
            web_client,
            live_mode,
            handle,
            internet_list_window_flag,
            resources,
            state_resetter,
        }
    }
    async fn request_playlist(
        current_category: LanguageCategory,
        web_client: Client,
        resources: Arc<RwLock<Vec<SingleResourcePane>>>,
        live_mode: Arc<AtomicBool>,
        state_resetter: Arc<StateResetter>,
    ) {
        let url = match &current_category {
            LanguageCategory::English => ENGLISH_PLAYLIST_URL,
            LanguageCategory::Chinese => CHINESE_PLAYLIST_URL,
            LanguageCategory::None => {
                warn!("None Category selected");
                return;
            }
        };

        match web_client.get(url).send().await {
            Ok(response) => {
                info!("get response success");
                let bytes_stream = response
                    .bytes_stream()
                    .map(|item| item.map_err(std::io::Error::other));

                let mut buf_reader =
                    BufReader::new(tokio_util::io::StreamReader::new(bytes_stream));
                let mut buf = vec![0; 1024 * 32];
                match buf_reader.read(&mut buf).await {
                    Ok(read_size) => {
                        let mut reader = quick_m3u8::Reader::from_bytes(
                            &buf[0..read_size],
                            ParsingOptions::default(),
                        );
                        let mut resources = resources.write().await;
                        resources.clear();
                        while let Ok(Some(hls_line)) = reader.read_line() {
                            if let quick_m3u8::HlsLine::Uri(uri) = hls_line {
                                let single_resource_pane = SingleResourcePane::new(
                                    state_resetter.clone(),
                                    MediaResource {
                                        name: uri.to_string(),
                                    },
                                    live_mode.clone(),
                                );
                                resources.push(single_resource_pane);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("read buffer error!{:?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("send request error!{:?}", e);
            }
        }
    }
}
impl Widget for &mut SelectableArea {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.ctx().input(|state| {
            if state.viewport().close_requested() {
                self.internet_list_window_flag
                    .store(false, std::sync::atomic::Ordering::Relaxed);
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
                if let Ok(mut resources) = self.resources.try_write() {
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
        if self.clicked {
            self.handle.spawn(SelectableArea::request_playlist(
                self.language_category.clone(),
                self.web_client.clone(),
                self.resources.clone(),
                self.live_mode.clone(),
                self.state_resetter.clone(),
            ));
            self.clicked = false;
        }
        ui.separator();
        ui.response()
    }
}

struct ScrollableArea {
    resources: Arc<RwLock<Vec<SingleResourcePane>>>,
}
impl ScrollableArea {
    fn new(resources: Arc<RwLock<Vec<SingleResourcePane>>>) -> Self {
        Self { resources }
    }
}
impl Widget for &mut ScrollableArea {
    fn ui(self, ui: &mut Ui) -> Response {
        ScrollArea::vertical().show(ui, |ui| {
            if let Ok(resources) = self.resources.try_read() {
                for resource_pane in &*resources {
                    ui.add(resource_pane);
                }
            }
        });
        ui.response()
    }
}

struct SingleResourcePane {
    state_resetter: Arc<StateResetter>,
    resource: MediaResource,
    live_mode: Arc<AtomicBool>,
}
impl SingleResourcePane {
    fn new(
        state_resetter: Arc<StateResetter>,
        resource: MediaResource,
        live_mode: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state_resetter,
            resource,
            live_mode,
        }
    }
}
impl Widget for &SingleResourcePane {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.set_min_height(100.0);
        let btn_response = ui.add(Button::new(&self.resource.name));
        if btn_response.clicked() {
            self.state_resetter
                .reset_media_input(Path::new(&self.resource.name));
            self.live_mode
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        ui.response()
    }
}
