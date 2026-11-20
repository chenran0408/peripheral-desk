//! Personal, editable copies of the M HUB frontend.
use crate::storage::atomic_write;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) const LOCAL_SITE: &str = "https://mhub.peripheraldesk.test/";
pub(super) fn root() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("找不到当前用户的应用数据目录")?;
    Ok(PathBuf::from(local)
        .join("PeripheralDesk")
        .join("MHubWorkspace"))
}

pub(super) fn build_name(name: &str) -> bool {
    name.strip_prefix("main-")
        .and_then(|name| name.strip_suffix(".js"))
        .is_some_and(|hash| {
            hash.len() == 8
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
}

pub(super) fn workspace_name(s: &str) -> bool {
    build_name(s) || super::builtin::folder_name(s)
}

pub(super) fn selected() -> Result<PathBuf, String> {
    let root = root()?;
    let build = fs::read_to_string(root.join("current.txt"))
        .map_err(|_| "请先安装驱动中心或选择已有的本地版本")?;
    if !workspace_name(build.trim()) {
        return Err("工作区版本记录无效".into());
    }
    let path = root.join(build.trim());
    if !path.join("index.html").is_file() {
        return Err("所选工作区缺少 index.html".into());
    }
    Ok(path)
}

pub(super) fn styles_path() -> Result<PathBuf, String> {
    let path = root()?.join("custom.css");
    if !path.exists() {
        atomic_write(&path, include_bytes!("custom.css"))?;
    }
    Ok(path)
}

pub(super) fn script_path() -> Result<PathBuf, String> {
    let path = root()?.join("personal.js");
    if !path.exists() {
        atomic_write(&path, include_bytes!("personal.js"))?;
    }
    Ok(path)
}

pub(super) fn prepare_user_hooks(dest: &Path) -> Result<(), String> {
    super::bootstrap::mount(dest)?;
    super::fixes::apply(dest)?;
    let source = root()?.join("personal.js");
    let generated = dest.join("_peripheral_desk").join("personal.js");
    match fs::read(&source) {
        Ok(content) => atomic_write(&generated, &content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if generated.exists() {
                fs::remove_file(generated).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn local_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
