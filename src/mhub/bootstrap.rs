//! Mount the local driver center in a previously saved editable workspace.
use crate::storage::atomic_write;
use std::{fs, path::Path};

const HEAD: &str = include_str!("hub/head.html");
const MARKER: &str = "name=\"peripheral-desk-driver-hub\"";

pub(super) fn mount(root: &Path) -> Result<(), String> {
    for (name, bytes) in [
        ("desk.css", include_bytes!("hub/desk.css").as_slice()),
        ("desk.js", include_bytes!("hub/desk.js").as_slice()),
    ] {
        let path = root.join("desk-hub").join(name);
        if !path.exists() {
            atomic_write(&path, bytes)?;
        }
    }
    let path = root.join("index.html");
    let mut html = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if !html.contains(MARKER) {
        let start = html
            .find("<head>")
            .ok_or("本地网页缺少 <head>，无法加载驱动中心")?;
        html.insert_str(start + "<head>".len(), HEAD);
        atomic_write(&path, html.as_bytes())?;
    }
    Ok(())
}
