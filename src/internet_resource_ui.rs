//! The internet_resource_ui module manages the ui of a seperate window
//! The ui is with respect to the content of online resources
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use egui::{Button, CentralPanel, ScrollArea, Ui, ViewportBuilder, ViewportId};
use futures_util::StreamExt;
use quick_m3u8::config::ParsingOptions;
use reqwest::Client;
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::RwLock,
};

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
    available_resource_map: Arc<RwLock<HashMap<LanguageCategory, VecDeque<MediaResource>>>>,
    current_category: Arc<RwLock<LanguageCategory>>,
    web_client: Client,
    change_input_ctx: Arc<RwLock<ResetInputContext>>,
    internet_list_window_flag: Arc<AtomicBool>,
}

impl InternetResourceUI {
    pub fn new(
        change_input_ctx: ResetInputContext,
        internet_list_window_flag: Arc<AtomicBool>,
    ) -> Self {
        let mut available_resource_map = HashMap::new();
        available_resource_map.insert(LanguageCategory::English, VecDeque::new());
        available_resource_map.insert(LanguageCategory::Chinese, VecDeque::new());
        let available_resource_map = Arc::new(RwLock::new(available_resource_map));
        let current_category = Arc::new(RwLock::new(LanguageCategory::None));
        let web_client = Client::new();
        let change_input_ctx = Arc::new(RwLock::new(change_input_ctx));
        Self {
            available_resource_map,
            current_category,
            web_client,
            change_input_ctx,
            internet_list_window_flag,
        }
    }
    pub fn show(&mut self, ui: &mut Ui) {
        let current_category = self.current_category.clone();
        let available_resource_map = self.available_resource_map.clone();
        let web_client = self.web_client.clone();
        let change_input_ctx = self.change_input_ctx.clone();
        let internet_list_window_flag = self.internet_list_window_flag.clone();
        ui.show_viewport_deferred(
            ViewportId::from_hash_of("Internet Resource UI"),
            ViewportBuilder::default(),
            move |ui, _viewport_class| {
                CentralPanel::default().show_inside(ui, |ui| {
                    if let Ok(mut current_category) = current_category.try_write() {
                        let selectable_area_res = ui
                            .horizontal(|ui| {
                                let res0 = ui.selectable_value(
                                    &mut *current_category,
                                    LanguageCategory::None,
                                    "None",
                                );
                                let res1 = ui.selectable_value(
                                    &mut *current_category,
                                    LanguageCategory::English,
                                    "English",
                                );
                                let res2 = ui.selectable_value(
                                    &mut *current_category,
                                    LanguageCategory::Chinese,
                                    "Chinese",
                                );
                                res0.clicked() || res1.clicked() || res2.clicked()
                            })
                            .inner;
                        ui.separator();
                        ScrollArea::vertical().show(ui, |ui| {
                            if *current_category != LanguageCategory::None
                                && let Ok(map) = available_resource_map.try_read()
                            {
                                let queue = &map[&*current_category];
                                if selectable_area_res
                                    && queue.is_empty()
                                    && let Ok(change_input_ctx) = change_input_ctx.try_read()
                                {
                                    change_input_ctx
                                        .runtime_handle
                                        .spawn(Self::request_playlist(
                                            available_resource_map.clone(),
                                            current_category.clone(),
                                            web_client.clone(),
                                        ));
                                }

                                for resource in queue {
                                    let btn_response = ui.add(Button::new(&resource.name));
                                    if btn_response.clicked()
                                        && let Ok(mut context) = change_input_ctx.try_write()
                                    {
                                        context.path = PathBuf::from(&resource.name);

                                        AppUI::reset_media_input(context.clone());

                                        context
                                            .live_mode
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                }
                            }
                        });
                    }
                });
                ui.ctx().input(|state| {
                    if state.viewport().close_requested() {
                        internet_list_window_flag
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            },
        );
    }
    async fn request_playlist(
        available_resource_map: Arc<RwLock<HashMap<LanguageCategory, VecDeque<MediaResource>>>>,
        current_category: LanguageCategory,
        web_client: Client,
    ) -> PlayerResult<()> {
        let mut map = available_resource_map.write().await;
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
        if let Some(queue) = map.get_mut(&current_category) {
            while let Ok(Some(hls_line)) = reader.read_line() {
                if let quick_m3u8::HlsLine::Uri(uri) = hls_line {
                    queue.push_back(MediaResource {
                        name: uri.to_string(),
                    });
                }
            }
        }

        Ok(())
    }
}
