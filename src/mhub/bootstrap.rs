//! Keep the official driver UI and CZ key layouts editable in the user's M HUB workspace.
use crate::{plugin_manager::download, storage::atomic_write};
use std::{collections::BTreeSet, fs, path::Path};

const CDN: &str = "https://cdn.mchose.com.cn/";
const SITE: &str = "https://www.mchose.com.cn/";
const CZ: &str = "cizhou/CZ_SHARED_DATA";
const MAX_FILE: usize = 40 * 1024 * 1024;

fn safe_path(path: &str) -> bool {
    path.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    })
}

// Both the page and its stylesheet contain absolute CDN URLs, some inside inline JavaScript.
fn cdn_files(source: &str, fonts: bool) -> BTreeSet<&str> {
    source
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '<' | '>'))
        .filter_map(|token| token.strip_prefix(CDN))
        .map(|path| path.split(['?', '#']).next().unwrap_or(path))
        .filter(|path| {
            safe_path(path)
                && if fonts {
                    path.starts_with("configCenter/pcAssets/fonts/")
                        && matches!(path.rsplit('.').next(), Some("woff2" | "woff" | "ttf"))
                } else {
                    (path.starts_with("customPage/driver-ui/")
                        && (path.ends_with(".css") || path.ends_with(".js")))
                        || (path.starts_with("configCenter/config/")
                            && path.ends_with("/globalLoader.js"))
                }
        })
        .collect()
}

fn fetch(base: &str, path: &str) -> Result<Vec<u8>, String> {
    let bytes = download(&format!("{base}{path}"), MAX_FILE)
        .map_err(|e| format!("下载 M HUB 资源 {path} 失败：{e}"))?;
    if !path.ends_with(".html")
        && bytes
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"<!doctype"))
    {
        return Err(format!("迈从服务器返回了网页，缺少资源 {path}"));
    }
    Ok(bytes)
}

fn save_cdn(dest: &Path, path: &str) -> Result<(), String> {
    let target = dest.join("vendor-cdn").join(path);
    if target.is_file() {
        return Ok(());
    }
    let bytes = fetch(CDN, path)?;
    if !path.ends_with(".css") {
        return atomic_write(&target, &bytes);
    }
    let css = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut local_css = css.clone();
    for font in cdn_files(&css, true) {
        save_cdn(dest, font)?;
        local_css = local_css.replace(&format!("{CDN}{font}"), &format!("/vendor-cdn/{font}"));
    }
    atomic_write(&target, local_css.as_bytes())
}

fn cz_assets(data: &[u8]) -> Result<Vec<String>, String> {
    let rows: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(data).map_err(|e| format!("CZ 键盘资源清单无效：{e}"))?;
    let mut files = BTreeSet::new();
    for (key, value) in rows {
        // The nested entrypoints index refers to the same files already listed by name.
        if key == "entrypoints" {
            continue;
        }
        let path = value
            .as_str()
            .ok_or_else(|| format!("CZ 资源 {key} 缺少路径"))?;
        if !safe_path(path) {
            return Err(format!("CZ 资源路径无效：{path}"));
        }
        files.insert(path.to_owned());
    }
    Ok(files.into_iter().collect())
}

fn save_cz(dest: &Path) -> Result<(), String> {
    let manifest_path = format!("{CZ}/asset-manifest.json");
    let manifest_file = dest.join(&manifest_path);
    let manifest = if manifest_file.is_file() {
        fs::read(&manifest_file).map_err(|e| e.to_string())?
    } else {
        let data = fetch(SITE, &manifest_path)?;
        atomic_write(&manifest_file, &data)?;
        data
    };
    let files = cz_assets(&manifest)?;
    for batch in files.chunks(6) {
        std::thread::scope(|scope| -> Result<(), String> {
            let jobs: Vec<_> = batch
                .iter()
                .map(|name| {
                    scope.spawn(move || {
                        let path = format!("{CZ}/{name}");
                        let target = dest.join(&path);
                        if !target.is_file() {
                            atomic_write(&target, &fetch(SITE, &path)?)?;
                        }
                        Ok::<(), String>(())
                    })
                })
                .collect();
            for job in jobs {
                job.join()
                    .map_err(|_| "CZ 键盘资源下载中断".to_string())??;
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub(super) fn populate(dest: &Path, html: &str) -> Result<String, String> {
    let mut local = html.to_owned();
    for path in cdn_files(html, false) {
        save_cdn(dest, path)?;
        local = local.replace(&format!("{CDN}{path}"), &format!("/vendor-cdn/{path}"));
    }
    if html.contains("cizhou/CZ_SHARED_DATA/loader.js") {
        save_cz(dest)?;
    }
    Ok(local)
}
