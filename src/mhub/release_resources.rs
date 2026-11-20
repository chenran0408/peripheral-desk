//! Install the driver-center resources published with this exact app release.
use crate::plugin_manager::download;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const PROJECT: &str = "chenran0408/peripheral-desk";
const MAX_PACKAGE: usize = 96 << 20;
const NOTICES: [&str; 3] = ["MHUB-NOTICE.txt", "MHUB-SOURCE.md", "MiSans-License.pdf"];

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    size: usize,
    browser_download_url: String,
    digest: Option<String>,
}

fn release_asset(json: &[u8], version: &str) -> Result<(Asset, String), String> {
    let release: Release = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let tag = format!("v{version}");
    let name = format!("PeripheralDesk-{version}-mhub-resources.zip");
    if release.draft || release.tag_name != tag {
        return Err("发布版本与当前外设盒不符".into());
    }
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("外设盒 {version} 的驱动中心资源尚未发布"))?;
    let url = format!("https://github.com/{PROJECT}/releases/download/{tag}/{name}");
    if asset.browser_download_url != url || asset.size == 0 || asset.size > MAX_PACKAGE {
        return Err("驱动中心发布包的来源或大小不符".into());
    }
    let hash = asset
        .digest
        .as_deref()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("GitHub 未提供该资源包的 SHA-256 校验值")?
        .to_ascii_lowercase();
    Ok((asset, hash))
}

fn unpack(bytes: &[u8], root: &Path) -> Result<PathBuf, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage = root.join(format!(".resources-{}-{stamp}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        for name in ["manifest.json", "payload.zip"].into_iter().chain(NOTICES) {
            let mut entry = zip
                .by_name(name)
                .map_err(|_| format!("驱动中心资源包缺少 {name}"))?;
            let limit = if name == "payload.zip" {
                MAX_PACKAGE as u64
            } else {
                4 << 20
            };
            let mut file = fs::File::create_new(stage.join(name)).map_err(|e| e.to_string())?;
            if io::copy(&mut entry.by_ref().take(limit + 1), &mut file)
                .map_err(|e| e.to_string())?
                > limit
            {
                return Err(format!("驱动中心资源包中的 {name} 太大"));
            }
        }
        let dest = super::builtin::prepare(&stage, root)?;
        let notices = dest.join("_peripheral_desk").join("notices");
        fs::create_dir_all(&notices).map_err(|e| e.to_string())?;
        for name in NOTICES {
            let notice = notices.join(name);
            if !notice.exists() {
                fs::rename(stage.join(name), notice).map_err(|e| e.to_string())?;
            }
        }
        crate::storage::atomic_write(
            &notices.join("app-version.txt"),
            env!("CARGO_PKG_VERSION").as_bytes(),
        )?;
        Ok(dest)
    })();
    let _ = fs::remove_dir_all(stage);
    result
}

pub(super) fn install(root: &Path) -> Result<PathBuf, String> {
    let version = env!("CARGO_PKG_VERSION");
    let url = format!("https://api.github.com/repos/{PROJECT}/releases/tags/v{version}");
    let json = download(&url, 2 << 20).map_err(|e| format!("无法检查驱动中心版本：{e}"))?;
    let (asset, hash) = release_asset(&json, version)?;
    let bytes = download(&asset.browser_download_url, asset.size)
        .map_err(|e| format!("无法下载驱动中心：{e}"))?;
    if bytes.len() != asset.size || format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err("资源包的 SHA-256 校验失败，已保留原版本".into());
    }
    unpack(&bytes, root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_release_resources_require_a_matching_url_and_hash() {
        let tag = "v0.5.0-preview.2";
        let name = "PeripheralDesk-0.5.0-preview.2-mhub-resources.zip";
        let digest = "e4".repeat(32);
        let mut json = serde_json::json!({"tag_name":tag,"draft":false,"assets":[{"name":name,"size":400,"browser_download_url":format!("https://github.com/{PROJECT}/releases/download/{tag}/{name}"),"digest":format!("sha256:{digest}")}]});
        assert_eq!(
            release_asset(json.to_string().as_bytes(), &tag[1..])
                .unwrap()
                .1,
            digest
        );
        assert!(release_asset(json.to_string().as_bytes(), "0.5.0").is_err());
        json["assets"][0]["digest"] = serde_json::Value::Null;
        assert!(release_asset(json.to_string().as_bytes(), &tag[1..]).is_err());
    }
    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for &(name, bytes) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn resource_install_keeps_licenses_and_reuses_an_edited_workspace() {
        let payload = archive(&[
            ("index.html", b"first"),
            ("assets/main-Abc12_XY.js", b"test"),
        ]);
        let manifest = serde_json::json!({"schema":1,"build":"main-Abc12_XY.js","sha256":format!("{:x}", Sha256::digest(&payload))}).to_string();
        let zip = archive(&[
            ("manifest.json", manifest.as_bytes()),
            ("payload.zip", &payload),
            (NOTICES[0], b"attribution"),
            (NOTICES[1], b"source"),
            (NOTICES[2], b"font license"),
        ]);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pd-resource-{}-{stamp}", std::process::id()));
        let ready = unpack(&zip, &root).unwrap();
        let data = ready.join("_peripheral_desk/notices");
        assert_eq!(fs::read(data.join(NOTICES[1])).unwrap(), b"source");
        assert_eq!(
            fs::read_to_string(data.join("app-version.txt")).unwrap(),
            env!("CARGO_PKG_VERSION")
        );
        fs::write(ready.join("index.html"), b"personal edit").unwrap();
        assert_eq!(unpack(&zip, &root).unwrap(), ready);
        assert_eq!(
            fs::read(ready.join("index.html")).unwrap(),
            b"personal edit"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
