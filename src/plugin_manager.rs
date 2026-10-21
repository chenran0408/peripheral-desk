//! Versioned driver packages, verified installation and isolated native execution.
use crate::{hid::wide, protocol::Result, storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::c_void,
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::{null, null_mut},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    Networking::WinHttp::*,
    System::LibraryLoader::*,
};
pub const ABI_VERSION: u32 = 1;
const REPOSITORY: &str = "https://github.com/chenran0408/peripheral-desk-drivers";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub download_bytes: u64,
    pub package_url: String,
    pub sha256: String,
    pub dll_sha256: String,
    #[serde(default = "abi_one")]
    pub abi: u32,
    #[serde(default = "core_min")]
    pub core_min: String,
    #[serde(default = "core_max")]
    pub core_max_exclusive: String,
    #[serde(default = "stable")]
    pub channel: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub changelog: String,
    pub device: Value,
}
fn abi_one() -> u32 {
    1
}
fn core_min() -> String {
    "0.4.0".into()
}
fn core_max() -> String {
    "0.5.0".into()
}
fn stable() -> String {
    "stable".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageManifest {
    pub schema: u32,
    pub abi: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dll: String,
    pub dll_sha256: String,
    #[serde(default = "core_min")]
    pub core_min: String,
    #[serde(default = "core_max")]
    pub core_max_exclusive: String,
    #[serde(default = "stable")]
    pub channel: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub changelog: String,
    pub device: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstalledDriver {
    pub id: String,
    pub version: String,
    pub manifest: PackageManifest,
    pub directory: String,
    #[serde(default)]
    pub previous: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Catalog {
    schema: u32,
    #[serde(default)]
    channel: String,
    drivers: Vec<DriverPackage>,
}
fn root() -> PathBuf {
    crate::backend::data_path()
        .parent()
        .unwrap()
        .join("drivers")
}
fn safe_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Err("驱动标识格式无效".into())
    } else {
        Ok(())
    }
}
fn safe_version(v: &str) -> Result<()> {
    if v.is_empty()
        || v.len() > 64
        || !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || v.ends_with('.')
    {
        return Err("驱动版本格式无效".into());
    }
    version(v)?;
    let normalized = v.strip_prefix('v').unwrap_or(v);
    let base = normalized.split('-').next().unwrap_or("");
    if base
        .split('.')
        .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("驱动版本需要三个数字段".into());
    }
    if normalized
        .split_once('-')
        .is_some_and(|(_, pre)| pre.split('.').any(str::is_empty))
    {
        return Err("驱动预览版本格式无效".into());
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn is_hash(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())
}
fn version(v: &str) -> Result<(u32, u32, u32)> {
    let mut p = v
        .trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or("")
        .split('.');
    let a = p
        .next()
        .ok_or("缺少版本号")?
        .parse()
        .map_err(|_| "版本号无效")?;
    let b = p
        .next()
        .ok_or("版本号无效")?
        .parse()
        .map_err(|_| "版本号无效")?;
    let c = p
        .next()
        .ok_or("版本号无效")?
        .parse()
        .map_err(|_| "版本号无效")?;
    if p.next().is_some() {
        return Err("版本号无效".into());
    }
    Ok((a, b, c))
}
fn compatible(abi: u32, min: &str, max: &str) -> Result<()> {
    let current = version(env!("CARGO_PKG_VERSION"))?;
    if abi != ABI_VERSION || current < version(min)? || current >= version(max)? {
        Err(format!(
            "驱动需要主程序 {min} 至 {max}（不含），接口版本 {abi}；请先更新主程序"
        ))
    } else {
        Ok(())
    }
}
pub fn channel() -> String {
    std::fs::read_to_string(root().join("channel.txt"))
        .ok()
        .filter(|v| v == "preview")
        .unwrap_or_else(stable)
}
pub fn set_channel(value: &str) -> Result<()> {
    if !["stable", "preview"].contains(&value) {
        return Err("更新通道无效".into());
    }
    storage::atomic_write(&root().join("channel.txt"), value.as_bytes())
}
fn cached_catalog() -> Result<Option<Vec<u8>>> {
    let pointer = root().join(format!("catalog-{}.pointer", channel()));
    let hash = match std::fs::read_to_string(pointer) {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !is_hash(&hash) {
        return Err("驱动目录缓存指针无效".into());
    }
    let path = root().join("catalog-cache").join(format!("{hash}.json"));
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let signature = std::fs::read(path.with_extension("json.sig")).map_err(|e| e.to_string())?;
    verify_catalog_signature(&bytes, &signature)?;
    Ok(Some(bytes))
}
fn cache_catalog(bytes: &[u8], signature: &[u8]) -> Result<()> {
    verify_catalog_signature(bytes, signature)?;
    let hash = digest(bytes);
    let path = root().join("catalog-cache").join(format!("{hash}.json"));
    storage::atomic_write(&path, bytes)?;
    storage::atomic_write(&path.with_extension("json.sig"), signature)?;
    storage::atomic_write(
        &root().join(format!("catalog-{}.pointer", channel())),
        hash.as_bytes(),
    )
}
fn parse_catalog(bytes: &[u8]) -> Result<Vec<DriverPackage>> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("驱动目录过大".into());
    }
    let catalog: Catalog =
        serde_json::from_slice(bytes).map_err(|e| format!("驱动目录无效：{e}"))?;
    if (!catalog.channel.is_empty() && catalog.channel != channel())
        || catalog.schema != 1
        || catalog.drivers.len() > 512
    {
        return Err("驱动目录版本或数量不支持".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in &catalog.drivers {
        let metadata: crate::devices::DeviceManifest =
            serde_json::from_value(p.device.clone()).map_err(|e| format!("设备描述无效：{e}"))?;
        metadata.validate()?;
        safe_id(&p.id)?;
        safe_version(&p.version)?;
        if !ids.insert(p.id.clone())
            || !is_hash(&p.sha256)
            || !is_hash(&p.dll_sha256)
            || p.download_bytes > 64 * 1024 * 1024
            || !p
                .package_url
                .starts_with(&format!("{REPOSITORY}/releases/download/"))
            || p.channel != channel()
            || p.device.get("key").and_then(Value::as_str) != Some(&p.id)
        {
            return Err("驱动目录包含未受信任或无效的包".into());
        }
    }
    Ok(catalog.drivers)
}
pub fn catalog() -> Result<Vec<DriverPackage>> {
    if let Some(bytes) = cached_catalog()? {
        return parse_catalog(&bytes);
    }
    if channel() == "preview" {
        return Ok(vec![]);
    }
    parse_catalog(include_bytes!("../assets/driver-catalog.json"))
}
pub fn refresh_catalog() -> Result<Vec<DriverPackage>> {
    let url = format!(
        "{REPOSITORY}/releases/download/catalog-{0}/catalog-{0}.json",
        channel()
    );
    let bytes = download(&url, 2 * 1024 * 1024)?;
    let signature = download(&format!("{url}.sig"), 1024)?;
    verify_catalog_signature(&bytes, &signature)?;
    let entries = parse_catalog(&bytes)?;
    cache_catalog(&bytes, &signature)?;
    Ok(entries)
}
pub fn import_catalog(path: &Path) -> Result<Vec<DriverPackage>> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let signature = std::fs::read(path.with_extension("json.sig"))
        .map_err(|_| "离线目录缺少同名.json.sig签名文件")?;
    verify_catalog_signature(&bytes, &signature)?;
    let entries = parse_catalog(&bytes)?;
    cache_catalog(&bytes, &signature)?;
    Ok(entries)
}
fn pointer(id: &str) -> PathBuf {
    root().join(id).join("installed.json")
}
pub fn installed(id: &str) -> Result<Option<InstalledDriver>> {
    safe_id(id)?;
    let bytes = match std::fs::read(pointer(id)) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let current: InstalledDriver =
        serde_json::from_slice(&bytes).map_err(|e| format!("已安装驱动记录无效：{e}"))?;
    if current.id != id
        || current.manifest.id != id
        || current.version != current.manifest.version
        || current.directory != current.version
        || current.manifest.dll != "driver.dll"
        || current.manifest.schema != 1
    {
        return Err("驱动安装位置或标识不匹配".into());
    }
    safe_version(&current.directory)?;
    if let Some(previous) = &current.previous {
        safe_version(previous)?;
    }
    if !root()
        .join(id)
        .join(&current.directory)
        .join("driver.dll")
        .is_file()
    {
        return Ok(None);
    }
    Ok(Some(current))
}
/// Every valid on-disk installation, including drivers absent from the selected channel.
pub fn installed_all() -> Result<Vec<InstalledDriver>> {
    let entries = match std::fs::read_dir(root()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut drivers = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if safe_id(&id).is_err() {
            continue;
        }
        if let Some(driver) = installed(&id)? {
            drivers.push(driver);
        }
    }
    drivers.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(drivers)
}
pub fn is_installed(id: &str) -> bool {
    installed(id).ok().flatten().is_some()
}
pub fn artwork(id: &str) -> Option<PathBuf> {
    installed(id)
        .ok()
        .flatten()
        .map(|d| root().join(id).join(d.directory).join("device.png"))
        .filter(|p| p.is_file())
}
pub fn installed_descriptors() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter_map(|id| installed(&id).ok().flatten())
        .map(|d| d.manifest.device)
        .collect()
}
fn backup_device_tree(source: &Path, destination: &Path) -> Result<()> {
    use std::{fs, os::windows::fs::MetadataExt};
    fn ordinary(path: &Path) -> Result<std::fs::Metadata> {
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0 {
            return Err("设备配置备份拒绝链接或重解析目录，旧驱动和配置保持不变".into());
        }
        Ok(metadata)
    }
    fn ancestors(path: &Path) -> Result<()> {
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            ordinary(ancestor)?;
        }
        Ok(())
    }
    fn copy(
        source: &Path,
        destination: &Path,
        source_root: &Path,
        backup_root: &Path,
    ) -> Result<()> {
        let metadata = ordinary(source)?;
        let canonical_source = fs::canonicalize(source).map_err(|e| e.to_string())?;
        if !canonical_source.starts_with(source_root) {
            return Err("设备配置备份源路径越界，未更新驱动".into());
        }
        if let Ok(metadata) = fs::symlink_metadata(destination) {
            if metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0 {
                return Err("设备配置备份目标不允许链接或重解析点".into());
            }
        }
        let parent = destination.parent().ok_or("设备配置备份目标无父目录")?;
        ancestors(parent)?;
        if !fs::canonicalize(parent)
            .map_err(|e| e.to_string())?
            .starts_with(backup_root)
        {
            return Err("设备配置备份目标路径越界，未更新驱动".into());
        }
        if metadata.is_dir() {
            fs::create_dir_all(destination).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                copy(
                    &entry.path(),
                    &destination.join(entry.file_name()),
                    source_root,
                    backup_root,
                )?;
            }
        } else if metadata.is_file() {
            let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
        } else {
            return Err("设备配置包含不支持备份的特殊文件，未更新驱动".into());
        }
        Ok(())
    }
    match fs::symlink_metadata(source) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(_) => {}
    }
    ancestors(source)?;
    if !ordinary(source)?.is_dir() {
        return Err("设备配置路径不是普通目录，未更新驱动".into());
    }
    let parent = destination.parent().ok_or("设备配置备份目标无父目录")?;
    ancestors(parent)?;
    let source_root = fs::canonicalize(source).map_err(|e| e.to_string())?;
    let backup_root = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    if backup_root.starts_with(&source_root)
        || fs::canonicalize(destination).is_ok_and(|p| p.starts_with(&source_root))
    {
        return Err("设备配置备份不能位于源配置目录内".into());
    }
    copy(source, destination, &source_root, &backup_root)
}
fn backup(id: &str) -> Result<()> {
    let data = crate::backend::data_path().parent().unwrap().to_owned();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let target = data.join("backups").join(id).join(stamp.to_string());
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(&data).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if e.file_type().map_err(|e| e.to_string())?.is_file()
            && e.path().extension().is_some_and(|s| s == "json")
        {
            std::fs::copy(e.path(), target.join(e.file_name())).map_err(|e| e.to_string())?;
        }
    }
    let device = data.join("devices").join(id);
    backup_device_tree(&device, &target.join("device"))?;
    Ok(())
}

pub fn install(id: &str) -> Result<InstalledDriver> {
    let p = catalog()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or("驱动目录没有此设备")?;
    compatible(p.abi, &p.core_min, &p.core_max_exclusive)?;
    let bytes = download(&p.package_url, 64 * 1024 * 1024)?;
    install_bytes(&p, &bytes)
}
pub fn install_offline(path: &Path) -> Result<InstalledDriver> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("离线包过大".into());
    }
    let hash = digest(&bytes);
    let mut entries = catalog()?;
    if !entries.iter().any(|p| p.sha256.eq_ignore_ascii_case(&hash)) {
        if let Some(parent) = path.parent() {
            let index = parent.join(format!("catalog-{}.json", channel()));
            if index.is_file() {
                entries = import_catalog(&index)?;
            }
        }
    }
    let p = entries
        .into_iter()
        .find(|p| p.sha256.eq_ignore_ascii_case(&hash))
        .ok_or("离线包不在当前受信驱动目录中；请先导入发布页附带的目录文件")?;
    install_bytes(&p, &bytes)
}
fn install_bytes(package: &DriverPackage, bytes: &[u8]) -> Result<InstalledDriver> {
    safe_id(&package.id)?;
    safe_version(&package.version)?;
    compatible(package.abi, &package.core_min, &package.core_max_exclusive)?;
    if bytes.len() as u64 != package.download_bytes
        || !digest(bytes).eq_ignore_ascii_case(&package.sha256)
    {
        return Err("下载包大小或 SHA-256 校验失败，旧驱动保持不变".into());
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if zip.len() > 128 {
        return Err("驱动包文件过多".into());
    }
    let manifest_bytes = {
        let mut f = zip
            .by_name("manifest.json")
            .map_err(|_| "驱动包缺少manifest.json")?;
        let mut v = Vec::new();
        f.by_ref()
            .take(256 * 1024 + 1)
            .read_to_end(&mut v)
            .map_err(|e| e.to_string())?;
        if v.len() > 256 * 1024 {
            return Err("驱动描述过大".into());
        }
        v
    };
    let manifest: PackageManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    if manifest.schema != 1
        || manifest.abi != package.abi
        || manifest.id != package.id
        || manifest.version != package.version
        || manifest.dll != "driver.dll"
        || manifest.dll_sha256 != package.dll_sha256
        || manifest.device != package.device
        || manifest.core_min != package.core_min
        || manifest.core_max_exclusive != package.core_max_exclusive
        || manifest.channel != package.channel
        || manifest.capabilities != package.capabilities
        || manifest.changelog != package.changelog
        || manifest.name != package.name
        || manifest.description != package.description
    {
        return Err("驱动包描述与受信目录不一致".into());
    }
    compatible(
        manifest.abi,
        &manifest.core_min,
        &manifest.core_max_exclusive,
    )?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let incoming = root().join(&package.id).join(format!("incoming-{stamp}"));
    std::fs::create_dir_all(&incoming).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut total = 0u64;
        for n in 0..zip.len() {
            let mut f = zip.by_index(n).map_err(|e| e.to_string())?;
            let path = f.enclosed_name().ok_or("驱动包路径越界")?;
            if path.components().any(|part| {
                if let std::path::Component::Normal(name) = part {
                    let name = name.to_string_lossy();
                    name.contains(':')
                        || name.ends_with('.')
                        || name.ends_with(' ')
                        || matches!(
                            name.split('.')
                                .next()
                                .unwrap_or("")
                                .to_ascii_uppercase()
                                .as_str(),
                            "CON"
                                | "PRN"
                                | "AUX"
                                | "NUL"
                                | "COM1"
                                | "COM2"
                                | "COM3"
                                | "COM4"
                                | "COM5"
                                | "COM6"
                                | "COM7"
                                | "COM8"
                                | "COM9"
                                | "LPT1"
                                | "LPT2"
                                | "LPT3"
                                | "LPT4"
                                | "LPT5"
                                | "LPT6"
                                | "LPT7"
                                | "LPT8"
                                | "LPT9"
                        )
                } else {
                    true
                }
            }) || f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            {
                return Err("驱动包含非法路径或链接".into());
            }
            total = total.checked_add(f.size()).ok_or("驱动包过大")?;
            if total > 128 * 1024 * 1024 {
                return Err("驱动包解压体积过大".into());
            }
            let dest = incoming.join(path);
            if f.is_dir() {
                std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dest)
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut out).map_err(|e| e.to_string())?;
            out.sync_all().map_err(|e| e.to_string())?;
        }
        let dll = std::fs::read(incoming.join("driver.dll")).map_err(|e| e.to_string())?;
        if !digest(&dll).eq_ignore_ascii_case(&manifest.dll_sha256) {
            return Err("驱动DLL校验失败，旧版保持不变".into());
        }
        probe_staged(&incoming.join("driver.dll"), &manifest.dll_sha256)?;
        crate::resident_macros::disable_device(&package.id)?;
        backup(&package.id)?;
        let previous = installed(&package.id)?.and_then(|p| {
            if p.directory == package.version {
                p.previous
            } else {
                Some(p.directory)
            }
        });
        let destination = root().join(&package.id).join(&package.version);
        if destination.exists() {
            let old = std::fs::read(destination.join("driver.dll")).map_err(|e| e.to_string())?;
            if digest(&old) != digest(&dll) {
                return Err("相同版本的驱动内容不一致，拒绝覆盖".into());
            }
            std::fs::remove_dir_all(&incoming).map_err(|e| e.to_string())?;
        } else {
            std::fs::rename(&incoming, &destination).map_err(|e| e.to_string())?;
        }
        let current = InstalledDriver {
            id: package.id.clone(),
            version: package.version.clone(),
            manifest,
            directory: package.version.clone(),
            previous: previous.filter(|v| v != &package.version),
        };
        storage::atomic_write(
            &pointer(&package.id),
            &serde_json::to_vec_pretty(&current).map_err(|e| e.to_string())?,
        )?;
        Ok(current)
    })();
    if result.is_err() && incoming.exists() {
        let _ = std::fs::remove_dir_all(&incoming);
    }
    result
}
pub fn rollback(id: &str) -> Result<InstalledDriver> {
    crate::resident_macros::disable_device(id)?;
    let current = installed(id)?.ok_or("尚未安装此驱动")?;
    let old = current.previous.as_ref().ok_or("没有可回退的旧版本")?;
    safe_version(old)?;
    let manifest: PackageManifest = serde_json::from_slice(
        &std::fs::read(root().join(id).join(old).join("manifest.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if manifest.id != id
        || manifest.version != *old
        || manifest.dll != "driver.dll"
        || manifest.schema != 1
    {
        return Err("旧版驱动记录不匹配".into());
    }
    compatible(
        manifest.abi,
        &manifest.core_min,
        &manifest.core_max_exclusive,
    )?;
    let bytes =
        std::fs::read(root().join(id).join(old).join("driver.dll")).map_err(|e| e.to_string())?;
    if digest(&bytes) != manifest.dll_sha256 {
        return Err("旧版驱动校验失败".into());
    }
    backup(id)?;
    let restored = InstalledDriver {
        id: id.into(),
        version: old.clone(),
        directory: old.clone(),
        manifest,
        previous: Some(current.directory),
    };
    storage::atomic_write(
        &pointer(id),
        &serde_json::to_vec_pretty(&restored).map_err(|e| e.to_string())?,
    )?;
    Ok(restored)
}
pub fn uninstall(id: &str) -> Result<()> {
    crate::resident_macros::disable_device(id)?;
    safe_id(id)?;
    backup(id)?;
    let directory = root().join(id);
    if directory.is_dir() {
        std::fs::remove_dir_all(directory).map_err(|e| e.to_string())?;
    }
    Ok(())
}
struct Http(*mut c_void);
impl Drop for Http {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}
pub fn download(url: &str, limit: usize) -> Result<Vec<u8>> {
    unsafe {
        let rest = url.strip_prefix("https://").ok_or("仅允许HTTPS下载")?;
        let (host, path) = rest.split_once('/').ok_or("下载地址无效")?;
        if host.is_empty()
            || host
                .bytes()
                .any(|b| b <= 32 || b >= 127 || b == b'@' || b == b':' || b == b'\\')
        {
            return Err("下载主机无效".into());
        }
        let session = Http(WinHttpOpen(
            wide("PeripheralDesk/0.4").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        WinHttpSetTimeouts(session.0, 15000, 15000, 20000, 30000);
        let connection = Http(WinHttpConnect(session.0, wide(host).as_ptr(), 443, 0));
        if connection.0.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let request = Http(WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide(&format!("/{path}")).as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(format!("网络下载失败：{}", std::io::Error::last_os_error()));
        }
        let mut status = 0u32;
        let mut size = 4u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        ) == 0
            || status != 200
        {
            return Err(format!("服务器返回 {status}，已安装驱动仍可使用"));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 32768];
        loop {
            let mut n = 0;
            if WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut n,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            if n == 0 {
                break;
            }
            if bytes.len() + n as usize > limit {
                return Err("下载超出允许体积".into());
            }
            bytes.extend_from_slice(&chunk[..n as usize]);
        }
        Ok(bytes)
    }
}
#[repr(C)]
struct Buffer {
    ptr: *mut u8,
    len: usize,
}
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
fn invoke_native(key: &str, request: &Value) -> Result<Value> {
    let driver = installed(key)?.ok_or("尚未安装此设备驱动，请打开驱动仓库下载")?;
    compatible(
        driver.manifest.abi,
        &driver.manifest.core_min,
        &driver.manifest.core_max_exclusive,
    )?;
    let dll = root().join(key).join(&driver.directory).join("driver.dll");
    let bytes = std::fs::read(&dll).map_err(|e| e.to_string())?;
    if digest(&bytes) != driver.manifest.dll_sha256 {
        return Err("设备驱动校验失败，请重新安装".into());
    }
    unsafe {
        let lib = Library(LoadLibraryExW(
            wide(&dll.to_string_lossy()).as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        ));
        if lib.0.is_null() {
            return Err(format!("驱动加载失败：{}", std::io::Error::last_os_error()));
        }
        let abi: unsafe extern "C" fn() -> u32 = std::mem::transmute(
            GetProcAddress(lib.0, c"pd_driver_abi".as_ptr().cast()).ok_or("驱动缺少接口版本")?,
        );
        if abi() != ABI_VERSION {
            return Err("驱动接口版本不兼容".into());
        }
        let call: unsafe extern "C" fn(*const u8, usize) -> Buffer = std::mem::transmute(
            GetProcAddress(lib.0, c"pd_driver_request".as_ptr().cast())
                .ok_or("驱动缺少请求接口")?,
        );
        let free: unsafe extern "C" fn(*mut u8, usize) = std::mem::transmute(
            GetProcAddress(lib.0, c"pd_driver_free".as_ptr().cast()).ok_or("驱动缺少释放接口")?,
        );
        let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let output = call(input.as_ptr(), input.len());
        if output.ptr.is_null() || output.len > 4 * 1024 * 1024 {
            if !output.ptr.is_null() {
                free(output.ptr, output.len);
            }
            return Err("设备驱动返回长度无效".into());
        }
        let value = serde_json::from_slice(std::slice::from_raw_parts(output.ptr, output.len))
            .map_err(|e| format!("驱动返回格式无效：{e}"));
        free(output.ptr, output.len);
        let reply: Value = value?;
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("设备驱动请求失败")
                .into());
        }
        Ok(reply.get("value").cloned().unwrap_or(Value::Null))
    }
}
pub fn worker_main(key: &str) {
    let outcome = (|| {
        safe_id(key)?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 256 * 1024 {
            return Err("驱动请求过大".into());
        }
        let request: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        invoke_native(key, &request)
    })();
    let reply = match outcome {
        Ok(value) => json!({"ok":true,"value":value,"error":""}),
        Err(error) => json!({"ok":false,"value":null,"error":error}),
    };
    if let Ok(bytes) = serde_json::to_vec(&reply) {
        let _ = std::io::stdout().write_all(&bytes);
    }
}
pub fn invoke(key: &str, mut request: Value) -> Result<Value> {
    safe_id(key)?;
    if !is_installed(key) {
        return Err("尚未安装此设备驱动，请打开驱动仓库下载".into());
    }
    let context = crate::backend::data_path()
        .parent()
        .unwrap()
        .join("devices")
        .join(key);
    if let Some(map) = request.as_object_mut() {
        map.insert("config_dir".into(), json!(context));
    }
    let input = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if input.len() > 256 * 1024 {
        return Err("驱动请求过大".into());
    }
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--plugin-worker", key])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut output = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .by_ref()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(bytes)
    });
    let send = child.stdin.take().unwrap().write_all(&input);
    if let Err(e) = send {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(e.to_string());
    }
    let operation = request.get("op").and_then(Value::as_str).unwrap_or("");
    let timeout = if key == "mchose-g75-pro" && operation == "controls-apply" {
        360
    } else if key == "hecate-g2-pro" {
        if operation.contains("apply") {
            20
        } else {
            12
        }
    } else if operation.contains("apply") {
        120
    } else {
        30
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = reader.join();
                    return Err(
                        "设备驱动异常退出，其他设备仍可使用；本次写入恢复情况无法确认".into(),
                    );
                }
                break;
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(e.to_string());
            }
        }
        if start.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("设备驱动响应超时，已隔离结束；本次写入恢复情况无法确认".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let bytes = reader.join().map_err(|_| "驱动结果读取线程异常")??;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("驱动结果超出体积限制".into());
    }
    let reply: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("设备请求失败")
            .into());
    }
    let value = reply.get("value").cloned().unwrap_or(Value::Null);
    if matches!(
        operation,
        "read-state"
            | "controls-read"
            | "controls-apply"
            | "lighting-read"
            | "lighting-apply"
            | "fx-query"
    ) {
        let name = if operation.starts_with("controls") {
            "controls"
        } else if operation.starts_with("lighting") {
            "lighting"
        } else if operation == "fx-query" {
            "effects"
        } else {
            "state"
        };
        storage::write_json_preserving(&context.join(format!("{name}.json")), value.clone())?;
    }
    Ok(value)
}

fn probe_native(path: &Path, expected: &str) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if digest(&bytes) != expected {
        return Err("预检驱动校验失败".into());
    }
    unsafe {
        let lib = Library(LoadLibraryExW(
            wide(&path.to_string_lossy()).as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        ));
        if lib.0.is_null() {
            return Err(format!(
                "驱动依赖或格式错误：{}",
                std::io::Error::last_os_error()
            ));
        }
        let abi: unsafe extern "C" fn() -> u32 = std::mem::transmute(
            GetProcAddress(lib.0, c"pd_driver_abi".as_ptr().cast()).ok_or("缺少ABI接口")?,
        );
        if abi() != ABI_VERSION {
            return Err("驱动ABI版本不匹配".into());
        }
        for name in [c"pd_driver_request", c"pd_driver_free"] {
            if GetProcAddress(lib.0, name.as_ptr().cast()).is_none() {
                return Err("驱动导出不完整".into());
            }
        }
    }
    Ok(())
}
pub fn probe_worker_main(path: &Path, expected: &str) {
    if let Err(error) = probe_native(path, expected) {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
fn probe_staged(path: &Path, expected: &str) -> Result<()> {
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--plugin-probe")
        .arg(path)
        .arg(expected)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                return if s.success() {
                    Ok(())
                } else {
                    Err("驱动隔离预检失败，旧版本保持可用".into())
                }
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("驱动预检超时，未激活新版本".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn verify_catalog_signature(bytes: &[u8], signature: &[u8]) -> Result<()> {
    use windows_sys::Win32::Security::Cryptography::*;
    unsafe {
        let mut algorithm = null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_RSA_ALGORITHM, null(), 0) != 0 {
            return Err("无法验证驱动目录签名".into());
        }
        let mut key = null_mut();
        let public = include_bytes!("../assets/driver-catalog.pub");
        let imported = BCryptImportKeyPair(
            algorithm,
            null_mut(),
            BCRYPT_RSAPUBLIC_BLOB,
            &mut key,
            public.as_ptr(),
            public.len() as u32,
            0,
        );
        if imported != 0 {
            BCryptCloseAlgorithmProvider(algorithm, 0);
            return Err("驱动目录公钥无效".into());
        }
        let padding = BCRYPT_PKCS1_PADDING_INFO {
            pszAlgId: BCRYPT_SHA256_ALGORITHM,
        };
        let hash = Sha256::digest(bytes);
        let status = BCryptVerifySignature(
            key,
            (&padding as *const BCRYPT_PKCS1_PADDING_INFO).cast(),
            hash.as_ptr(),
            hash.len() as u32,
            signature.as_ptr(),
            signature.len() as u32,
            BCRYPT_PAD_PKCS1,
        );
        BCryptDestroyKey(key);
        BCryptCloseAlgorithmProvider(algorithm, 0);
        if status != 0 {
            return Err("驱动目录签名无效，未安装或执行任何新驱动".into());
        }
        Ok(())
    }
}

pub fn check_core_compatibility(candidate: &str) -> Result<()> {
    let app = std::env::current_exe().map_err(|e| e.to_string())?;
    check_core_compatibility_at(app.parent().ok_or("主程序目录无效")?, candidate)
}
pub fn check_core_compatibility_at(app_root: &Path, candidate: &str) -> Result<()> {
    let target = version(candidate)?;
    let drivers = app_root.join("data").join("drivers");
    let entries = match std::fs::read_dir(&drivers) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if safe_id(&id).is_err() {
            continue;
        }
        let path = drivers.join(&id).join("installed.json");
        if !path.exists() {
            continue;
        }
        let current: InstalledDriver =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if current.id != id
            || current.manifest.id != id
            || current.directory != current.version
            || current.version != current.manifest.version
            || current.manifest.schema != 1
            || current.manifest.dll != "driver.dll"
        {
            return Err("已安装插件记录无效".into());
        }
        safe_version(&current.directory)?;
        if let Some(previous) = &current.previous {
            safe_version(previous)?;
        }
        let m = current.manifest;
        if m.abi != ABI_VERSION
            || target < version(&m.core_min)?
            || target >= version(&m.core_max_exclusive)?
        {
            return Err(format!(
                "{} 的现有插件与主程序 {candidate} 不兼容；请先更新兼容的设备插件",
                m.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod backup_tests {
    use super::*;
    #[test]
    fn nested_unknown_device_files_are_backed_up_without_changing_source() {
        let base = std::env::var_os("PD_UPDATE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "plugin-backup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("devices/example-device");
        let target = root.join("backup");
        std::fs::create_dir_all(source.join("profiles/custom/empty")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let unknown = [0u8, 255, 17, 128, 0];
        std::fs::write(source.join("settings.json"), b"{\"future\":true}").unwrap();
        std::fs::write(source.join("profiles/custom/unknown.bin"), unknown).unwrap();
        backup_device_tree(&source, &target.join("device")).unwrap();
        assert_eq!(
            std::fs::read(target.join("device/settings.json")).unwrap(),
            b"{\"future\":true}"
        );
        assert_eq!(
            std::fs::read(target.join("device/profiles/custom/unknown.bin")).unwrap(),
            unknown
        );
        assert!(target.join("device/profiles/custom/empty").is_dir());
        assert_eq!(
            std::fs::read(source.join("profiles/custom/unknown.bin")).unwrap(),
            unknown
        );
        assert!(source.join("settings.json").is_file());
        assert!(backup_device_tree(&source, &source).is_err());
        assert!(backup_device_tree(&source, &source.join("recursive-backup")).is_err());
        // The fixture root is a newly created direct child of the named base.
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        assert!(canonical_root.starts_with(std::fs::canonicalize(&base).unwrap()));
        assert_eq!(
            canonical_root.parent(),
            Some(std::fs::canonicalize(&base).unwrap().as_path())
        );
        std::fs::remove_dir_all(canonical_root).unwrap();
    }
}
