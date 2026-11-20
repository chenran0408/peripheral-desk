//! Mount the local driver center in a previously saved editable workspace.
use crate::storage::atomic_write;
use std::{fs, path::Path};

const HEAD: &str = include_str!("hub/head.html");
const MARKER: &str = "name=\"peripheral-desk-driver-hub\"";
const SWITCH_SCRIPT: &str = r#"<script src="/desk-hub/switch-device.js"></script>"#;

pub(super) fn mount(root: &Path) -> Result<(), String> {
    for (name, bytes) in [
        ("desk.css", include_bytes!("hub/desk.css").as_slice()),
        ("desk.js", include_bytes!("hub/desk.js").as_slice()),
        (
            "switch-device.js",
            include_bytes!("hub/switch-device.js").as_slice(),
        ),
    ] {
        let path = root.join("desk-hub").join(name);
        if !path.exists() {
            atomic_write(&path, bytes)?;
        }
    }
    let path = root.join("index.html");
    let mut html = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let insertion = if !html.contains(MARKER) {
        HEAD
    } else if !html.contains("/desk-hub/switch-device.js") {
        SWITCH_SCRIPT
    } else {
        return Ok(());
    };
    let start = html
        .find("<head>")
        .ok_or("本地网页缺少 <head>，无法加载驱动中心")?;
    html.insert_str(start + "<head>".len(), insertion);
    atomic_write(&path, html.as_bytes())?;
    Ok(())
}
