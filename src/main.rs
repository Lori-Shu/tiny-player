#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
#![deny(unused)]
#![deny(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
};

use eframe::{
    egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew},
    wgpu::{Backends, InstanceDescriptor},
};
use egui::{IconData, ImageSource, Vec2, include_image};

use tracing::{Level, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod ai_sub_title;
mod appui;
mod audio_play;
mod decode;
mod present_data_manage;

const WINDOW_ICON: ImageSource = include_image!("../resources/play.ico");
static CURRENT_EXE_PATH: LazyLock<PlayerResult<PathBuf>> = LazyLock::new(|| {
    let path = std::env::current_exe()?;
    Ok(path)
});

pub type PlayerResult<T> = anyhow::Result<T>;

/// main fun init log, init main ui type Appui
fn main() {
    unsafe {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    let targets_filter = tracing_subscriber::filter::Targets::default()
        .with_default(Level::WARN)
        .with_target("tiny_player", Level::INFO);

    let subscriber = tracing_subscriber::registry::Registry::default()
        .with(
            tracing_subscriber::fmt::layer()
                .with_thread_ids(true)
                .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339()),
        )
        .with(targets_filter);
    subscriber.init();
    let span = tracing::span!(Level::INFO, "main");
    let _main_entered = span.enter();
    info!("enter main span");
    if let Ok(tiny_app_ui) = appui::AppUi::new() {
        let mut options = eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            wgpu_options: WgpuConfiguration {
                wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew {
                    instance_descriptor: InstanceDescriptor {
                        backends: Backends::VULKAN,
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        if let ImageSource::Bytes { bytes, .. } = WINDOW_ICON {
            if let Ok(img) = image::load_from_memory(&bytes) {
                options.viewport.icon = Some(Arc::new(IconData {
                    width: img.width(),
                    height: img.height(),
                    rgba: img.as_bytes().to_vec(),
                }));
            }
        }
        options.centered = true;
        options.viewport.inner_size = Some(Vec2::new(900.0, 700.0));

        if let Err(e) = eframe::run_native(
            "tiny player",
            options,
            Box::new(|cc| {
                egui_extras::install_image_loaders(&cc.egui_ctx);
                tiny_app_ui.replace_fonts(&cc.egui_ctx);
                Ok(Box::new(tiny_app_ui))
            }),
        ) {
            warn!("eframe start error {}", e.to_string());
        }
    } else if let Err(e) = appui::AppUi::new() {
        warn!("appui construct err {}", e.to_string());
    }
}
