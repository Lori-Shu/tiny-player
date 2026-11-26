//! The resources module manages the static resources
//! which are included in output binaries

use egui::{ImageSource, include_image};

pub static VIDEO_FILE_IMG: ImageSource = include_image!("../resources/file-play.svg");
pub static VOLUME_IMG: ImageSource = include_image!("../resources/volume-1.svg");
pub static PLAY_IMG: ImageSource = include_image!("../resources/circle-play.svg");
pub static PAUSE_IMG: ImageSource = include_image!("../resources/circle-pause.svg");
pub static FULLSCREEN_IMG: ImageSource = include_image!("../resources/fullscreen.svg");
pub static PLAY_LIST_IMG: ImageSource = include_image!("../resources/list-video.svg");
pub static SUBTITLE_IMG: ImageSource = include_image!("../resources/dock.svg");
pub static TV_IMG: ImageSource = include_image!("../resources/tv.svg");
pub static DEFAULT_BG_IMG: &[u8] = include_bytes!("../resources/background_3.png");
pub static DEFAULT_COVER: &[u8] = include_bytes!("../resources/default_cover.png");
pub static MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
pub static EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
