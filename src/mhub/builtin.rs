//! Install the frontend supplied next to the executable into an editable, versioned workspace.
use super::workspace;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Seek},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
struct Manifest {
    schema: u32,
    build: String,
    sha256: String,
}

pub(super) fn package() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("mhub-builtin");
    dir.exists().then_some(dir)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn folder_name(name: &str) -> bool {
    name.strip_prefix("builtin-").is_some_and(digest)
}

fn extract<R: Read + Seek>(input: R, stage: &Path, build: &str) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(input).map_err(|e| e.to_string())?;
    let mut remaining = 256 << 20;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = entry.enclosed_name().ok_or("内置网页包含无效路径")?;
        if entry.is_symlink()
            || !path.components().all(|component| match component {
                Component::Normal(part) => {
                    let name = part.to_string_lossy();
                    let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                    !name.contains(':')
                        && !name.ends_with(['.', ' '])
                        && !matches!(
                            base.as_str(),
                            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                        )
                        && !base
                            .strip_prefix("COM")
                            .or_else(|| base.strip_prefix("LPT"))
                            .is_some_and(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_digit())
                }
                _ => false,
            })
        {
            return Err("内置网页包含无效路径".into());
        }
        let target = stage.join(path);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        let size = entry.size();
        if size > remaining {
            return Err("内置网页解压体积超出限制".into());
        }
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut out = File::create_new(target).map_err(|e| e.to_string())?;
        if io::copy(&mut entry.by_ref().take(size + 1), &mut out).map_err(|e| e.to_string())?
            != size
        {
            return Err("内置网页文件大小不符".into());
        }
        remaining -= size;
    }
    if !stage.join("index.html").is_file() || !stage.join("assets").join(build).is_file() {
        return Err("内置网页缺少入口文件".into());
    }
    Ok(())
}

pub(super) fn prepare(source: &Path, root: &Path) -> Result<PathBuf, String> {
    let manifest: Manifest = serde_json::from_reader(
        File::open(source.join("manifest.json"))
            .map_err(|e| format!("无法读取内置网页清单：{e}"))?,
    )
    .map_err(|e| format!("内置网页清单无效：{e}"))?;
    if manifest.schema != 1 || !workspace::build_name(&manifest.build) || !digest(&manifest.sha256)
    {
        return Err("内置网页版本或校验值无效".into());
    }
    let dest = root.join(format!("builtin-{}", manifest.sha256));
    if dest.join("index.html").is_file() {
        return Ok(dest);
    }
    let mut payload =
        File::open(source.join("payload.zip")).map_err(|e| format!("无法读取内置网页包：{e}"))?;
    let mut hash = Sha256::new();
    io::copy(&mut payload, &mut hash).map_err(|e| e.to_string())?;
    if format!("{:x}", hash.finalize()) != manifest.sha256 {
        return Err("内置网页校验失败，请重新安装包含网页的版本".into());
    }
    payload.rewind().map_err(|e| e.to_string())?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage = root.join(format!(".builtin-{}-{stamp}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = extract(payload, &stage, &manifest.build).and_then(|()| {
        fs::rename(&stage, &dest)
            .or_else(|e| {
                if dest.join("index.html").is_file() {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|e| e.to_string())
    });
    if stage.is_dir() {
        let _ = fs::remove_dir_all(stage);
    }
    result.map(|()| dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn pack(dir: &Path, index: &[u8]) -> Vec<u8> {
        let build = "main-Abc12_XY.js";
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in [
            ("index.html".to_owned(), index),
            (format!("assets/{build}"), &b"test"[..]),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents).unwrap();
        }
        let data = zip.finish().unwrap().into_inner();
        let checksum = format!("{:x}", Sha256::digest(&data));
        let manifest = serde_json::json!({"schema":1,"build":build,"sha256":checksum});
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        fs::write(dir.join("payload.zip"), &data).unwrap();
        data
    }

    #[test]
    fn bundled_revisions_preserve_edits_and_reject_damaged_updates() {
        let temp = std::env::temp_dir().join(format!(
            "pd-mhub-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let package = temp.join("mhub-builtin");
        let root = temp.join("workspaces");
        fs::create_dir_all(&package).unwrap();
        pack(&package, b"first");
        let first = prepare(&package, &root).unwrap();
        fs::write(first.join("index.html"), b"my edits").unwrap();
        assert_eq!(prepare(&package, &root).unwrap(), first);
        let second_zip = pack(&package, b"second");
        fs::write(package.join("payload.zip"), b"broken").unwrap();
        assert!(prepare(&package, &root).unwrap_err().contains("校验失败"));
        fs::write(package.join("payload.zip"), second_zip).unwrap();
        let second = prepare(&package, &root).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(first.join("index.html")).unwrap(), b"my edits");
        assert_eq!(fs::read(second.join("index.html")).unwrap(), b"second");
        fs::remove_dir_all(temp).unwrap();
    }
}
