//! The resources module manages the static resources
//! which are included in output binaries

use egui::{ImageSource, include_image};
pub const VIDEO_FILE_IMG: ImageSource = include_image!("../resources/file-play.svg");
pub const VOLUME_IMG: ImageSource = include_image!("../resources/volume-1.svg");
pub const PLAY_IMG: ImageSource = include_image!("../resources/circle-play.svg");
pub const PAUSE_IMG: ImageSource = include_image!("../resources/circle-pause.svg");
pub const FULLSCREEN_IMG: ImageSource = include_image!("../resources/fullscreen.svg");
pub const DEFAULT_BG_IMG: ImageSource = include_image!("../resources/background_3.png");
pub const PLAY_LIST_IMG: ImageSource = include_image!("../resources/list-video.svg");
pub const SUBTITLE_IMG: ImageSource = include_image!("../resources/dock.svg");
pub const TV_IMG: ImageSource = include_image!("../resources/tv.svg");
pub const DEFAULT_COVER: ImageSource = include_image!("../resources/default_cover.png");
pub const MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
pub const EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
