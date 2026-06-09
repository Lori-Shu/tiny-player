//! The headbar_ui module manages a subarea of the user interface
//! The subarea includes information text and a few control widgets
//! (e.g., button to input a single file, button to open the playlist window)

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32},
    },
    time::Instant,
};

use egui::{AtomExt, Button, Color32, CornerRadius, Image, Stroke, Ui, Vec2};
use egui_file::FileDialog;
use tracing::info;
use typed_builder::TypedBuilder;

use crate::{
    appui::{AppUI, ResetInputContext},
    internet_resource_ui::InternetResourceUI,
    playlist_ui::PlayListUI,
    resources::{PLAY_LIST_IMG, TV_IMG, VIDEO_FILE_IMG},
};
#[derive(TypedBuilder)]
pub struct HeadbarUI {
    visible_num: Arc<AtomicU32>,
    visible_flag: Arc<AtomicBool>,
    open_file_dialog: FileDialog,
    reset_input_context: ResetInputContext,
    live_mode: Arc<AtomicBool>,
    last_fps_update_instant: Instant,
    fps_text_str: String,
    playlist_window_flag: Arc<AtomicBool>,
    playlist_ui: PlayListUI,
    internet_list_window_flag: Arc<AtomicBool>,
    internet_resource_ui: InternetResourceUI,
}
impl HeadbarUI {
    pub fn paint_file_btn(&mut self, ui: &mut Ui) {
        let visible_num =
            f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
        let btn_rect = Vec2::new(
            ui.ctx().content_rect().width() / 20.0,
            ui.ctx().content_rect().width() / 20.0,
        );
        let file_image_button = egui::Button::new(
            Image::from(VIDEO_FILE_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(btn_rect),
        )
        .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
        .stroke(Stroke::new(
            1.0,
            Color32::from_white_alpha((10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let file_img_btn_response = ui.add(file_image_button);

        if file_img_btn_response.hovered() {
            self.visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if file_img_btn_response.clicked() {
            self.open_file_dialog.open();
        }
        let mut file_path = None;

        self.open_file_dialog.show(ui.ctx());
        if self.open_file_dialog.selected()
            && let Some(p) = self.open_file_dialog.path()
        {
            info!("path selected{:#?}", p);
            file_path = Some(p.to_path_buf())
        }

        if let Some(p) = file_path {
            let mut ctx = self.reset_input_context.clone();
            ctx.path = p.clone();
            AppUI::reset_media_input(ctx);
            if let Some(p_str) = p.to_str() {
                self.live_mode
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                info!("accept file path{}", p_str);
            }
        }
    }
    pub fn paint_frame_info_text(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut orange_color = Color32::ORANGE.to_srgba_unmultiplied();
            orange_color[3] = 100;
            let now_ins = Instant::now();
            if let Some(dur) = now_ins.checked_duration_since(self.last_fps_update_instant)
                && dur.as_secs() > 0
            {
                let fps = ui.input(|input| 1.0 / input.stable_dt.min(0.1));
                self.fps_text_str = format!("fps:{}", fps as i32);
                self.last_fps_update_instant = now_ins;
            }
            let rich_text = egui::RichText::new(&self.fps_text_str)
                .color(Color32::from_rgba_unmultiplied(
                    orange_color[0],
                    orange_color[1],
                    orange_color[2],
                    orange_color[3],
                ))
                .size(30.0);
            let fps_button = egui::Button::new(rich_text).frame(false);
            ui.add(fps_button);

            let mut date_time_str = "date-time：".to_string();
            if let Ok(formatter) =
                time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
                && let Ok(local_date_time) = time::OffsetDateTime::now_local()
                && let Ok(formatted_date_time_str) = local_date_time.format(&formatter)
            {
                date_time_str.push_str(formatted_date_time_str.as_str());
            }
            let rich_text = egui::RichText::new(date_time_str)
                .color(Color32::from_rgba_unmultiplied(
                    orange_color[0],
                    orange_color[1],
                    orange_color[2],
                    orange_color[3],
                ))
                .size(30.0);
            let date_time_button = egui::Button::new(rich_text).frame(false);

            ui.add(date_time_button);
        });
    }
    pub fn paint_playlist_button(&mut self, ui: &mut Ui) {
        let visible_num =
            f32::from_bits(self.visible_num.load(std::sync::atomic::Ordering::Relaxed));
        let open_btn = Button::new(
            Image::from(PLAY_LIST_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(Vec2::new(50.0, 50.0)),
        )
        .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
        .stroke(Stroke::new(
            1.0,
            Color32::from_white_alpha((10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if btn_response.clicked() {
            self.playlist_window_flag
                .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self
            .playlist_window_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.playlist_ui.show(ui);
        }
        let open_btn = Button::new(
            Image::from(TV_IMG)
                .tint(Color32::from_white_alpha((255.0 * visible_num) as u8))
                .atom_size(Vec2::new(50.0, 50.0)),
        )
        .fill(egui::Color32::from_white_alpha((10.0 * visible_num) as u8))
        .stroke(Stroke::new(
            1.0,
            Color32::from_white_alpha((10.0 * visible_num) as u8),
        ))
        .corner_radius(CornerRadius::from(30));

        let btn_response = ui.add(open_btn);

        if btn_response.hovered() {
            self.visible_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if btn_response.clicked() {
            self.internet_list_window_flag
                .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self
            .internet_list_window_flag
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.internet_resource_ui.show(ui);
        }
    }
}
