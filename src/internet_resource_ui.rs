use std::{
    collections::{HashMap, VecDeque},
    io::ErrorKind,
    sync::Arc,
};

use egui::{Button, CentralPanel, ScrollArea, Ui, ViewportBuilder, ViewportId};
use futures_util::StreamExt;
use quick_m3u8::config::ParsingOptions;
use reqwest::Client;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    runtime::Handle,
    sync::RwLock,
};

use crate::PlayerResult;
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
#[derive(Debug)]
pub struct InternetResourceUI {
    available_resource_map: Arc<RwLock<HashMap<LanguageCategory, VecDeque<MediaResource>>>>,
    current_category: LanguageCategory,
    web_client: Client,
}

impl InternetResourceUI {
    pub fn new() -> Self {
        let mut available_resource_map = HashMap::new();
        available_resource_map.insert(LanguageCategory::English, VecDeque::new());
        available_resource_map.insert(LanguageCategory::Chinese, VecDeque::new());
        let available_resource_map = Arc::new(RwLock::new(available_resource_map));
        let current_category = LanguageCategory::None;
        let web_client = Client::new();
        Self {
            available_resource_map,
            current_category,
            web_client,
        }
    }
    pub fn show(&mut self, ui: &mut Ui, async_runtime: Handle) -> Option<MediaResource> {
        ui.show_viewport_immediate(
            ViewportId::from_hash_of("Internet Resource UI"),
            ViewportBuilder::default(),
            |ui, _viewport_class| {
                CentralPanel::default()
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut self.current_category,
                                LanguageCategory::None,
                                "None",
                            );
                            ui.selectable_value(
                                &mut self.current_category,
                                LanguageCategory::English,
                                "English",
                            );
                            ui.selectable_value(
                                &mut self.current_category,
                                LanguageCategory::Chinese,
                                "Chinese",
                            );
                        });
                        ui.separator();
                        ScrollArea::vertical().show(ui, |ui| {
                            if self.current_category != LanguageCategory::None {
                                if let Ok(map) = self.available_resource_map.try_read() {
                                    let queue = &map[&self.current_category];
                                    if queue.is_empty() {
                                        async_runtime.spawn(Self::request_playlist(
                                            self.available_resource_map.clone(),
                                            self.current_category.clone(),
                                            self.web_client.clone(),
                                        ));
                                    }

                                    for resource in queue {
                                        let btn_response = ui.add(Button::new(&resource.name));
                                        if btn_response.clicked() {
                                            return Some(resource.clone());
                                        }
                                    }
                                }
                            }
                            None
                        })
                    })
                    .inner
            },
        )
        .inner
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
            .map(|item| item.map_err(|e| std::io::Error::new(ErrorKind::Other, e)));

        let mut buf_reader = BufReader::new(tokio_util::io::StreamReader::new(bytes_stream));
        let mut playlist = String::new();
        let mut buf = String::with_capacity(1024);
        for _count in 0..50 {
            let line_len = buf_reader.read_line(&mut buf).await?;
            playlist.push_str(&buf[0..line_len]);
        }
        let mut reader = quick_m3u8::Reader::from_str(&playlist, ParsingOptions::default());
        if let Some(queue) = map.get_mut(&current_category) {
            loop {
                if let Ok(Some(hls_line)) = reader.read_line() {
                    match hls_line {
                        quick_m3u8::HlsLine::Uri(uri) => {
                            queue.push_back(MediaResource {
                                name: uri.to_string(),
                            });
                        }
                        _ => {}
                    }
                } else {
                    break;
                }
            }
        }

        Ok(())
    }
}
