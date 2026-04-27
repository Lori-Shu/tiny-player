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
    wgpu::{
        BackendOptions, Backends, DeviceDescriptor, Features, InstanceDescriptor, InstanceFlags,
        MemoryBudgetThresholds, PowerPreference,
    },
};
use egui::{IconData, ImageSource, Vec2, include_image};

use tracing::{Level, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// mod ai_sub_title;
mod appui;
mod audio_play;
mod decode;
mod gpu_post_process;
mod internet_resource_ui;
mod playlist_ui;
mod present_data_manage;
// mod translate;

const WINDOW_ICON: ImageSource = include_image!("../resources/play.ico");
static _CURRENT_EXE_PATH: LazyLock<PlayerResult<PathBuf>> = LazyLock::new(|| {
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

    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: WgpuConfiguration {
            wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew {
                instance_descriptor: InstanceDescriptor {
                    backends: Backends::VULKAN,
                    flags: InstanceFlags::default(),
                    memory_budget_thresholds: MemoryBudgetThresholds::default(),
                    backend_options: BackendOptions::default(),
                    display: None,
                },
                display_handle: None,
                power_preference: PowerPreference::default(),
                native_adapter_selector: None,
                device_descriptor: Arc::new(|_adapter| DeviceDescriptor {
                    required_features: Features::default() | Features::TEXTURE_FORMAT_16BIT_NORM,
                    ..Default::default()
                }),
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
            match appui::AppUI::new(cc) {
                Ok(tiny_app_ui) => {
                    tiny_app_ui.replace_fonts(&cc.egui_ctx);
                    Ok(Box::new(tiny_app_ui))
                }
                Err(e) => Err(e.into()),
            }
        }),
    ) {
        warn!("eframe start error {}", e.to_string());
    }
}
