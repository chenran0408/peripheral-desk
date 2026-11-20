//! A personal copy of the public web frontend. Only the chosen user's workspace stores vendor files.
use crate::{plugin_manager::download, storage::atomic_write};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) const LOCAL_SITE: &str = "https://mhub.peripheraldesk.test/";
const SITE: &str = "https://www.mchose.com.cn/";
const FILE_LIMIT: usize = 40 * 1024 * 1024;
pub(super) fn root() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("找不到当前用户的应用数据目录")?;
    Ok(PathBuf::from(local)
        .join("PeripheralDesk")
        .join("MHubWorkspace"))
}

// Vite uses content-hashed filenames for entry points, lazy imports and stylesheet dependencies.
fn public_asset(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(
        ext,
        "js" | "css"
            | "wasm"
            | "svg"
            | "png"
            | "webp"
            | "jpg"
            | "woff"
            | "woff2"
            | "ttf"
            | "gif"
            | "mp3"
            | "ico"
    ) {
        return false;
    }
    let bytes = stem.as_bytes();
    bytes.len() > 9
        && bytes[bytes.len() - 9] == b'-'
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

fn asset_refs(source: &str) -> BTreeSet<String> {
    source
        .split(['"', '\'', '`', '(', ')', '\n', '\r'])
        .filter_map(|token| {
            let token = token.trim();
            let name = token
                .strip_prefix("./assets/")
                .or_else(|| token.strip_prefix("/assets/"))
                .or_else(|| token.strip_prefix("./"))?;
            public_asset(name).then(|| format!("assets/{name}"))
        })
        .collect()
}

fn entry(html: &str) -> Result<String, String> {
    asset_refs(html)
        .into_iter()
        .find(|s| s.starts_with("assets/main-") && s.ends_with(".js"))
        .ok_or_else(|| "官网未提供可识别的 M HUB 网页入口".to_string())
}

pub(super) fn build_name(s: &str) -> bool {
    public_asset(s) && s.starts_with("main-") && s.ends_with(".js")
}

pub(super) fn selected() -> Result<PathBuf, String> {
    let root = root()?;
    let build = fs::read_to_string(root.join("current.txt")).map_err(|_| "请先下载可编辑工作区")?;
    if !build_name(build.trim()) {
        return Err("工作区版本记录无效".into());
    }
    let path = root.join(build.trim());
    if !path.join("index.html").is_file() {
        return Err("所选工作区缺少 index.html".into());
    }
    Ok(path)
}

pub(super) fn fetch(path: &str) -> Result<Vec<u8>, String> {
    let bytes = download(&format!("{SITE}{path}"), FILE_LIMIT)
        .map_err(|e| format!("{path} 下载失败：{e}"))?;
    if !path.is_empty()
        && (bytes.starts_with(b"<!doctype html") || bytes.starts_with(b"<!DOCTYPE html"))
    {
        return Err(format!("官网未提供资源 {path}"));
    }
    Ok(bytes)
}

/// Build each release in its own directory; previously edited files never get replaced.
pub(super) fn install(progress: impl Fn(usize, usize)) -> Result<PathBuf, String> {
    let raw = fetch("")?;
    let html = std::str::from_utf8(&raw).map_err(|e| e.to_string())?;
    let main = entry(html)?;
    let build = main.strip_prefix("assets/").unwrap();
    let root = root()?;
    let dest = root.join(build);
    let current = root.join("current.txt");
    if dest.join("index.html").is_file() {
        if !current.is_file() {
            atomic_write(&current, build.as_bytes())?;
        }
        return Ok(dest);
    }
    fs::create_dir_all(dest.join("assets")).map_err(|e| e.to_string())?;
    let mut next = asset_refs(html);
    if html.contains("cizhou/CZ_SHARED_DATA/loader.js") {
        next.insert("cizhou/CZ_SHARED_DATA/loader.js".into());
    }
    if html.contains("mchose.ico") {
        next.insert("mchose.ico".into());
    }
    let mut visited = BTreeSet::new();
    let mut done = 0;
    while !next.is_empty() {
        let batch: Vec<_> = std::mem::take(&mut next)
            .into_iter()
            .filter(|name| visited.insert(name.clone()))
            .collect();
        for group in batch.chunks(6) {
            let entries = std::thread::scope(|scope| {
                let jobs: Vec<_> = group
                    .iter()
                    .map(|name| {
                        let target = dest.join(name);
                        scope.spawn(move || -> Result<_, String> {
                            let contents = if target.is_file() {
                                fs::read(&target).map_err(|e| e.to_string())?
                            } else {
                                let bytes = fetch(name)?;
                                atomic_write(&target, &bytes)?;
                                bytes
                            };
                            Ok(if name.ends_with(".js") || name.ends_with(".css") {
                                asset_refs(
                                    std::str::from_utf8(&contents).map_err(|e| e.to_string())?,
                                )
                            } else {
                                BTreeSet::new()
                            })
                        })
                    })
                    .collect();
                jobs.into_iter()
                    .map(|job| job.join().map_err(|_| "资源下载任务中断".to_string())?)
                    .collect::<Result<Vec<_>, _>>()
            })?;
            for refs in entries {
                next.extend(refs.into_iter().filter(|p| !visited.contains(p)));
            }
            done += group.len();
            progress(done, visited.len() + next.len());
        }
    }
    let page = super::bootstrap::populate(&dest, html)?;
    atomic_write(&dest.join("index.html"), page.as_bytes())?;
    if !current.is_file() {
        atomic_write(&current, build.as_bytes())?;
    }
    Ok(dest)
}

pub(super) fn styles_path() -> Result<PathBuf, String> {
    let path = root()?.join("custom.css");
    if !path.exists() {
        atomic_write(&path, include_bytes!("custom.css"))?;
    }
    Ok(path)
}

pub(super) fn local_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vite_graph_includes_lazy_chunks_and_leaves_remote_services_online() {
        let html = r#"<script type="module" src="./assets/main-Abc12_XY.js"></script>"#;
        assert_eq!(entry(html).unwrap(), "assets/main-Abc12_XY.js");
        let js = r#"import("./keys-A1234567.js");const m=["./led-B1234567.css","https://cdn.example/foreign-12345678.js","../escape-12345678.js"]"#;
        assert_eq!(
            asset_refs(js).into_iter().collect::<Vec<_>>(),
            ["assets/keys-A1234567.js", "assets/led-B1234567.css"]
        );
    }
}
