//! The resources module manages the static resources
//! which are included in output binaries

use egui::{ImageSource, include_image};
pub const VIDEO_FILE_IMG: ImageSource = include_image!("../resources/file-play.png");
pub const VOLUME_IMG: ImageSource = include_image!("../resources/volume-2.png");
pub const PLAY_IMG: ImageSource = include_image!("../resources/play.png");
pub const PAUSE_IMG: ImageSource = include_image!("../resources/pause.png");
pub const FULLSCREEN_IMG: ImageSource = include_image!("../resources/fullscreen.png");
pub const DEFAULT_BG_IMG: ImageSource = include_image!("../resources/background_2.png");
pub const PLAY_LIST_IMG: ImageSource = include_image!("../resources/list-video.png");
pub const SUBTITLE_IMG: ImageSource = include_image!("../resources/captions.png");
pub const TV_IMG: ImageSource = include_image!("../resources/tv.png");
pub const MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
pub const EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
