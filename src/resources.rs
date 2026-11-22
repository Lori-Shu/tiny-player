//! The resources module manages the static resources
//! which are included in output binaries

use std::{borrow::Cow, sync::LazyLock};
pub static VIDEO_FILE_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://file-play.svg"),
        include_bytes!("../resources/file-play.svg"),
    )
});
pub static VOLUME_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://volume-1.svg"),
        include_bytes!("../resources/volume-1.svg"),
    )
});
pub static PLAY_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://circle-play.svg"),
        include_bytes!("../resources/circle-play.svg"),
    )
});
pub static PAUSE_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://circle-pause.svg"),
        include_bytes!("../resources/circle-pause.svg"),
    )
});
pub static FULLSCREEN_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://fullscreen.svg"),
        include_bytes!("../resources/fullscreen.svg"),
    )
});
pub static DEFAULT_BG_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://background_3.png"),
        include_bytes!("../resources/background_3.png"),
    )
});
pub static PLAY_LIST_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://list-video.svg"),
        include_bytes!("../resources/list-video.svg"),
    )
});
pub static SUBTITLE_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://dock.svg"),
        include_bytes!("../resources/dock.svg"),
    )
});
pub static TV_IMG: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://tv.svg"),
        include_bytes!("../resources/tv.svg"),
    )
});
pub static DEFAULT_COVER: LazyLock<(Cow<str>, &[u8])> = LazyLock::new(|| {
    (
        Cow::Borrowed("bytes://default_cover.png"),
        include_bytes!("../resources/default_cover.png"),
    )
});
pub static MAPLE_FONT: &[u8] = include_bytes!("../resources/fonts/MapleMono-CN-Regular.ttf");
pub static EMOJI_FONT: &[u8] = include_bytes!("../resources/fonts/seguiemj.ttf");
