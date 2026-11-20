//! Native equivalents of the bundled M HUB driver-ui light and dark tokens.
//! Translucent web surfaces are composed on their parent before GDI painting.
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Palette {
    pub bg: u32,
    pub panel: u32,
    pub field: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub dim: u32,
    pub accent: u32,
    pub accent_light: u32,
    pub primary: u32,
    pub primary_pressed: u32,
    pub on_primary: u32,
    pub accent_surface: u32,
    pub accent_border: u32,
    pub sidebar_bg: u32,
    pub green: u32,
    pub amber: u32,
    pub error: u32,
    pub hover: u32,
    pub pressed: u32,
}

const LIGHT: Palette = Palette {
    bg: 0xffe6e7ed,
    panel: 0xfff3f3f6,
    field: 0xffffffff,
    border: 0xffcfd0d6,
    text: 0xff171718,
    muted: 0xff535458,
    dim: 0xff62646a,
    accent: 0xff0053e2,
    accent_light: 0xff00aaff,
    primary: 0xff0053e2,
    primary_pressed: 0xff001e60,
    on_primary: 0xffffffff,
    accent_surface: 0xffd4e1f7,
    accent_border: 0xffa6c2f4,
    sidebar_bg: 0xfff3f3f6,
    green: 0xff167751,
    amber: 0xff92500c,
    error: 0xffba3028,
    hover: 0xffe4e8ef,
    pressed: 0xffd8dce3,
};

const DARK: Palette = Palette {
    bg: 0xff000000,
    panel: 0xff1a1a1a,
    field: 0xff303030,
    border: 0xff414141,
    text: 0xffe8e8e8,
    muted: 0xffaeaeae,
    dim: 0xff999999,
    accent: 0xff226bee,
    accent_light: 0xffa9ddf7,
    primary: 0xff226bee,
    primary_pressed: 0xff164da7,
    on_primary: 0xffffffff,
    accent_surface: 0xff1b263c,
    accent_border: 0xff334e7b,
    sidebar_bg: 0xff1a1a1a,
    green: 0xff69ce9d,
    amber: 0xfff5b86a,
    error: 0xffff8980,
    hover: 0xff313131,
    pressed: 0xff414141,
};

static DARK_MODE: AtomicBool = AtomicBool::new(false);

pub fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

pub fn palette() -> &'static Palette {
    if is_dark() {
        &DARK
    } else {
        &LIGHT
    }
}

fn path() -> std::path::PathBuf {
    crate::backend::data_path().with_file_name("appearance.txt")
}
pub fn load() {
    DARK_MODE.store(
        std::fs::read_to_string(path()).is_ok_and(|s| s.trim() == "dark"),
        Ordering::Relaxed,
    );
}
pub fn save(dark: bool) -> Result<(), String> {
    crate::storage::atomic_write(&path(), if dark { b"dark" } else { b"light" })?;
    DARK_MODE.store(dark, Ordering::Relaxed);
    Ok(())
}
