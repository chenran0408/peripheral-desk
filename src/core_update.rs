//! Verified core-only releases and a path-constrained, parent-bound update helper.
use crate::{plugin_manager, protocol::Result, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    mem::zeroed,
    os::windows::{fs::MetadataExt, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, Threading::*},
};

const REPO: &str = "https://github.com/chenran0408/peripheral-desk";
const RELEASES: &str =
    "https://api.github.com/repos/chenran0408/peripheral-desk/releases?per_page=100";
const LIMIT: usize = 64 * 1024 * 1024;
const PROTOCOL: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub available: bool,
    pub channel: String,
    pub tag: String,
}
#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    pub job: PathBuf,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct Version {
    base: [u64; 3],
    pre: Vec<String>,
}
impl Version {
    fn parse(value: &str) -> Result<Self> {
        let value = value.strip_prefix('v').unwrap_or(value);
        let main = value.split_once('+').map_or(value, |(main, _)| main);
        if main.ends_with('-') {
            return Err("预览版本号不能为空".into());
        }
        let (base, pre) = main.split_once('-').map_or((main, ""), |(a, b)| (a, b));
        let pieces = base.split('.').collect::<Vec<_>>();
        if pieces.len() != 3 {
            return Err("版本号需要 major.minor.patch".into());
        }
        let mut nums = [0; 3];
        for (i, n) in pieces.iter().enumerate() {
            if n.is_empty()
                || n.len() > 1 && n.starts_with('0')
                || !n.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("版本号格式无效".into());
            }
            nums[i] = n.parse().map_err(|_| "版本号过大")?;
        }
        let pre = if pre.is_empty() {
            Vec::new()
        } else {
            let p = pre.split('.').map(str::to_string).collect::<Vec<_>>();
            if p.iter().any(|s| {
                s.is_empty()
                    || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    || (s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()))
            }) {
                return Err("预览版本号格式无效".into());
            }
            p
        };
        Ok(Self { base: nums, pre })
    }
    fn compare(&self, other: &Self) -> Ordering {
        let base = self.base.cmp(&other.base);
        if base != Ordering::Equal {
            return base;
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        }
        for (a, b) in self.pre.iter().zip(&other.pre) {
            let numeric = |v: &str| v.bytes().all(|b| b.is_ascii_digit());
            let order = match (numeric(a), numeric(b)) {
                (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => a.cmp(b),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}
/// Semantic version ordering shared by the core and device update interfaces.
pub fn is_newer_version(candidate: &str, current: &str) -> Result<bool> {
    Ok(Version::parse(candidate)?.compare(&Version::parse(current)?) == Ordering::Greater)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn file_hash(path: &Path) -> Result<String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    f.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("更新文件超过允许体积".into());
    }
    Ok(hash(&bytes))
}
fn asset_name(name: &str, version: &str) -> bool {
    name.eq_ignore_ascii_case(&format!("PeripheralDesk-{version}-windows-x64.zip"))
        || name.eq_ignore_ascii_case(&format!("PeripheralDesk-Core-{version}-windows-x64.zip"))
}
fn escaped(value: &str) -> String {
    let mut s = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            s.push(b as char)
        } else {
            s.push_str(&format!("%{b:02X}"))
        }
    }
    s
}
fn trusted_url(info: &UpdateInfo) -> Result<()> {
    let prefix = format!("{REPO}/releases/download/{}/", escaped(&info.tag));
    let name = info
        .url
        .strip_prefix(&prefix)
        .ok_or("主程序更新地址不是本项目的正式发布资产")?;
    if !asset_name(name, &info.version)
        || info.tag.trim_start_matches('v') != info.version
        || info.url.contains('?')
        || info.url.contains('#')
        || !valid_hash(&info.sha256)
        || info.size == 0
        || info.size > LIMIT as u64
    {
        return Err("主程序更新的文件名、大小或 SHA-256 信息不可信".into());
    }
    Ok(())
}
pub fn channel() -> String {
    application()
        .ok()
        .and_then(|(root, _)| fs::read_to_string(root.join("data/core-channel.txt")).ok())
        .filter(|value| value == "preview")
        .unwrap_or_else(|| "stable".into())
}
pub fn set_channel(value: &str) -> Result<()> {
    if !matches!(value, "stable" | "preview") {
        return Err("主程序更新通道无效".into());
    }
    let (root, _) = application()?;
    let data = ensure_dir(&root, "data")?;
    storage::atomic_write(&data.join("core-channel.txt"), value.as_bytes())
}
fn choose_release(releases: Vec<Release>, channel: &str) -> Result<(Version, Release)> {
    let mut selected: Option<(Version, Release)> = None;
    for release in releases {
        if release.draft || channel == "stable" && release.prerelease {
            continue;
        }
        let Ok(version) = Version::parse(&release.tag_name) else {
            continue;
        };
        if channel == "stable" && !version.pre.is_empty() {
            continue;
        }
        if selected
            .as_ref()
            .is_none_or(|(previous, _)| version.compare(previous) == Ordering::Greater)
        {
            selected = Some((version, release));
        }
    }
    selected.ok_or("该通道尚无可用的正式发布版本".into())
}
pub fn check(channel: &str) -> Result<UpdateInfo> {
    if !matches!(channel, "stable" | "preview") {
        return Err("主程序更新通道无效".into());
    }
    let bytes = plugin_manager::download(RELEASES, 4 * 1024 * 1024)?;
    let releases: Vec<Release> =
        serde_json::from_slice(&bytes).map_err(|e| format!("GitHub 发布目录无效：{e}"))?;
    let (version, release) = choose_release(releases, channel)?;
    let available =
        version.compare(&Version::parse(env!("CARGO_PKG_VERSION"))?) == Ordering::Greater;
    let normalized = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();
    let asset = release
        .assets
        .into_iter()
        .find(|a| asset_name(&a.name, &normalized));
    let mut info = UpdateInfo {
        version: normalized,
        notes: release.body,
        url: String::new(),
        size: 0,
        sha256: String::new(),
        available,
        channel: channel.into(),
        tag: release.tag_name,
    };
    if let Some(asset) = asset {
        info.url = asset.browser_download_url;
        info.size = asset.size;
        info.sha256 = asset
            .digest
            .and_then(|d| d.strip_prefix("sha256:").map(str::to_string))
            .unwrap_or_default();
    }
    if available {
        trusted_url(&info).map_err(|e| {
            format!("找到新版本，但无法安全更新：{e}。发布资产必须提供 GitHub SHA-256 digest。")
        })?;
    }
    Ok(info)
}
#[derive(Serialize, Deserialize, Clone)]
struct Document {
    path: String,
    sha256: String,
    previous: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Job {
    schema: u32,
    old_pid: u32,
    old_created: u64,
    target: String,
    old_version: String,
    new_version: String,
    old_sha256: String,
    new_sha256: String,
    documents: Vec<Document>,
}
#[derive(Serialize, Deserialize)]
struct Backup {
    schema: u32,
    target: String,
    version: String,
    sha256: String,
    abi: u32,
}
#[derive(Deserialize)]
struct SelfCheck {
    #[serde(rename = "appVersion")]
    version: String,
    abi: u32,
    #[serde(rename = "updateProtocol")]
    protocol: u32,
}
fn stamp() -> Result<String> {
    Ok(format!(
        "u{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ))
}
fn canonical(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|e| format!("无法确认更新路径 {}：{e}", path.display()))
}
fn same(a: &Path, b: &Path) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}
fn no_reparse(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err(format!("更新路径不允许链接或重解析点：{}", path.display()));
    }
    Ok(())
}
fn ensure_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    fs::create_dir_all(&path).map_err(|e| format!("应用目录不可写：{e}"))?;
    no_reparse(&path)?;
    let full = canonical(&path)?;
    if !same(&full, &path) {
        return Err("更新目录离开了当前应用的可写工作目录".into());
    }
    Ok(full)
}
fn target_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("PeripheralDesk.exe")
        || name.eq_ignore_ascii_case("peripheral-desk.exe")
}
fn application() -> Result<(PathBuf, String)> {
    let exe = canonical(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("主程序文件名无效")?
        .to_string();
    if !target_name(&name) {
        return Err("仅支持更新标准命名的 PeripheralDesk.exe / peripheral-desk.exe".into());
    }
    Ok((exe.parent().ok_or("主程序目录无效")?.to_path_buf(), name))
}
fn create_job() -> Result<(PathBuf, PathBuf, String)> {
    let (root, target) = application()?;
    let data = ensure_dir(&root, "data")?;
    let updates = ensure_dir(&data, "updates")?;
    let job = ensure_dir(&updates, &stamp()?)?;
    Ok((root, job, target))
}
fn process_identity(pid: u32) -> Result<(PathBuf, u64)> {
    unsafe {
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if process.is_null() {
            return Err(format!(
                "无法核对原进程：{}",
                std::io::Error::last_os_error()
            ));
        }
        let result = (|| {
            let mut name = vec![0u16; 32768];
            let mut len = name.len() as u32;
            if QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut len) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut created = std::mem::zeroed();
            let mut exited = zeroed();
            let mut kernel = zeroed();
            let mut user = zeroed();
            if GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok((
                canonical(Path::new(&String::from_utf16_lossy(&name[..len as usize])))?,
                ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
            ))
        })();
        CloseHandle(process);
        result
    }
}
fn parent_pid() -> Result<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut okay = Process32FirstW(snapshot, &mut entry);
        let mut parent = None;
        while okay != 0 {
            if entry.th32ProcessID == std::process::id() {
                parent = Some(entry.th32ParentProcessID);
                break;
            }
            okay = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        parent.ok_or("无法确认更新助手的父进程".into())
    }
}
fn self_check(exe: &Path, job: &Path, expected: &str) -> Result<()> {
    let output = job.join("self-check.json");
    let file = File::create(&output).map_err(|e| e.to_string())?;
    let mut process = Command::new(exe)
        .arg("--self-check")
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法自检主程序：{e}"))?;
    let start = Instant::now();
    loop {
        match process.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err("主程序更新自检失败".into());
                }
                break;
            }
            Ok(None) => {}
            Err(e) => {
                let _ = process.kill();
                let _ = process.wait();
                return Err(e.to_string());
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = process.kill();
            let _ = process.wait();
            return Err("主程序更新自检超时，原程序保持不变".into());
        }
        thread::sleep(Duration::from_millis(30));
    }
    let mut bytes = Vec::new();
    File::open(output)
        .map_err(|e| e.to_string())?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8192 {
        return Err("主程序自检输出过大".into());
    }
    let check: SelfCheck =
        serde_json::from_slice(&bytes).map_err(|e| format!("主程序缺少兼容的更新自检接口：{e}"))?;
    if check.version != expected
        || check.abi != plugin_manager::ABI_VERSION
        || check.protocol != PROTOCOL
    {
        return Err("主程序版本、插件接口或更新助手协议与发布信息不匹配".into());
    }
    Ok(())
}
fn document_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("更新文档路径不合法".into());
    }
    let parts = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| p.contains(':') || p.contains('\\') || p.ends_with('.') || p.ends_with(' '))
    {
        return Err("更新文档路径不合法".into());
    }
    let top = parts[0].to_ascii_lowercase();
    let leaf = parts.last().unwrap().to_ascii_lowercase();
    let allowed = if parts.len() == 1 {
        matches!(
            leaf.as_str(),
            "readme.md"
                | "changelog.md"
                | "sources.md"
                | "license"
                | "license.txt"
                | "notice"
                | "notice.txt"
        )
    } else {
        matches!(top.as_str(), "docs" | "third_party_licenses")
            && (leaf.ends_with(".md")
                || leaf.ends_with(".txt")
                || leaf.ends_with(".pdf")
                || (top == "docs" && (leaf.ends_with(".yml") || leaf.ends_with(".yaml")))
                || (top == "third_party_licenses"
                    && leaf.starts_with("license")
                    && !leaf.contains('.'))
                || (top == "third_party_licenses" && leaf == "license-apache-2.0")
                || (top == "third_party_licenses"
                    && matches!(leaf.as_str(), "manifest.json" | "unlicense"))
                || leaf == "license"
                || (top == "third_party_licenses" && leaf == "copying"))
    };
    if !allowed {
        return Err(format!("更新包包含非主程序/文档路径：{}", path.display()));
    }
    Ok(path.to_path_buf())
}
fn safe_target(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("替换路径超出应用目录".into());
        };
        path.push(part);
        if path.exists() {
            no_reparse(&path)?;
        }
    }
    if !path.starts_with(root) {
        return Err("替换路径超出应用目录".into());
    }
    Ok(path)
}
fn extract(info: &UpdateInfo, bytes: &[u8], job: &Path) -> Result<Vec<String>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if zip.len() > 192 {
        return Err("主程序更新包文件过多".into());
    }
    let mut seen = BTreeSet::new();
    let mut documents = Vec::new();
    let mut main = false;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err("更新包不能包含符号链接".into());
        }
        let full = file.enclosed_name().ok_or("更新包包含越界路径")?;
        let mut parts = full.components().collect::<Vec<_>>();
        if let Some(Component::Normal(name)) = parts.first() {
            let name = name.to_string_lossy();
            if name.eq_ignore_ascii_case(&format!("PeripheralDesk-{}-windows-x64", info.version))
                || name.eq_ignore_ascii_case(&format!(
                    "PeripheralDesk-Core-{}-windows-x64",
                    info.version
                ))
            {
                parts.remove(0);
            }
        }
        if file.is_dir() {
            continue;
        }
        let relative = parts.iter().collect::<PathBuf>();
        let name = relative
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("更新文件名无效")?;
        let executable = parts.len() == 1 && target_name(name);
        let path = if executable {
            PathBuf::from("candidate.exe")
        } else {
            PathBuf::from("documents").join(document_path(&relative.to_string_lossy())?)
        };
        let key = path.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            return Err("更新包含重复文件".into());
        }
        if file.size() > LIMIT as u64 {
            return Err("更新文件过大".into());
        }
        total = total.checked_add(file.size()).ok_or("更新文件体积溢出")?;
        if total > LIMIT as u64 {
            return Err("更新包解压后过大".into());
        }
        let mut data = Vec::new();
        file.by_ref()
            .take(LIMIT as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() > LIMIT {
            return Err("更新文件过大".into());
        }
        storage::atomic_write(&job.join(path), &data)?;
        if executable {
            if main {
                return Err("更新包含多个主程序".into());
            }
            main = true;
        } else {
            documents.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    if !main {
        return Err("更新包没有标准命名的主程序 EXE".into());
    }
    Ok(documents)
}
fn backup_configs(
    source: &Path,
    destination: &Path,
    root: bool,
    total: &mut u64,
    count: &mut usize,
) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    no_reparse(source)?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        no_reparse(&path)?;
        let name = entry.file_name();
        if root && matches!(name.to_str(), Some("updates" | "backups")) {
            continue;
        }
        let target = destination.join(&name);
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            backup_configs(&path, &target, false, total, count)?;
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "json" | "toml" | "ini" | "txt" | "xml"
            )
        }) {
            let length = entry.metadata().map_err(|e| e.to_string())?.len();
            *total = total.checked_add(length).ok_or("配置备份过大")?;
            *count += 1;
            if *total > 32 * 1024 * 1024 || *count > 2048 {
                return Err("用户配置备份超出允许体积或数量，未开始替换".into());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::copy(&path, &target).map_err(|e| format!("备份配置失败：{e}"))?;
        }
    }
    Ok(())
}
fn finalize_prepare(
    root: &Path,
    job: &Path,
    target: String,
    new_version: String,
    documents: Vec<String>,
) -> Result<PreparedUpdate> {
    let original = root.join(&target);
    no_reparse(&original)?;
    let old_sha256 = file_hash(&original)?;
    fs::copy(&original, job.join("original.exe")).map_err(|e| format!("备份原主程序失败：{e}"))?;
    let mut doc_records = Vec::new();
    for path in documents {
        let relative = document_path(&path)?;
        let current = safe_target(root, &relative)?;
        let previous = if current.exists() {
            let digest = file_hash(&current)?;
            let backup = job.join("old-documents").join(&relative);
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::copy(&current, &backup).map_err(|e| e.to_string())?;
            Some(digest)
        } else {
            None
        };
        doc_records.push(Document {
            sha256: file_hash(&job.join("documents").join(relative))?,
            path,
            previous,
        });
    }
    backup_configs(
        &root.join("data"),
        &job.join("config-backup"),
        true,
        &mut 0,
        &mut 0,
    )?;
    let (pid_path, old_created) = process_identity(std::process::id())?;
    if !same(&pid_path, &original) {
        return Err("准备更新的进程与目标主程序不匹配".into());
    }
    let new_sha256 = file_hash(&job.join("candidate.exe"))?;
    fs::copy(job.join("candidate.exe"), job.join("helper.exe")).map_err(|e| e.to_string())?;
    let definition = Job {
        schema: PROTOCOL,
        old_pid: std::process::id(),
        old_created,
        target,
        old_version: env!("CARGO_PKG_VERSION").into(),
        new_version,
        old_sha256,
        new_sha256,
        documents: doc_records,
    };
    let path = job.join("job.json");
    storage::atomic_write(
        &path,
        &serde_json::to_vec_pretty(&definition).map_err(|e| e.to_string())?,
    )?;
    Ok(PreparedUpdate { job: path })
}
pub fn prepare(info: &UpdateInfo) -> Result<PreparedUpdate> {
    plugin_manager::check_core_compatibility(&info.version)?;
    if Version::parse(&info.version)?.compare(&Version::parse(env!("CARGO_PKG_VERSION"))?)
        != Ordering::Greater
    {
        return Err("当前主程序已是最新，不会自动降级或重装".into());
    }
    trusted_url(info)?;
    let bytes = plugin_manager::download(&info.url, LIMIT)?;
    prepare_downloaded(info, &bytes)
}
fn prepare_downloaded(info: &UpdateInfo, bytes: &[u8]) -> Result<PreparedUpdate> {
    if bytes.len() as u64 != info.size || !hash(bytes).eq_ignore_ascii_case(&info.sha256) {
        return Err("主程序更新包大小或 SHA-256 校验失败，原程序保持不变".into());
    }
    let (root, job, target) = create_job()?;
    let documents = extract(info, bytes, &job)?;
    self_check(&job.join("candidate.exe"), &job, &info.version)?;
    finalize_prepare(&root, &job, target, info.version.clone(), documents)
}
fn read_backup(root: &Path) -> Result<Option<(Backup, PathBuf)>> {
    let directory = root.join("data").join("backups").join("current-core");
    if !directory.exists() {
        return Ok(None);
    }
    for path in [
        root.join("data"),
        root.join("data/backups"),
        directory.clone(),
    ] {
        no_reparse(&path)?;
    }
    no_reparse(&directory.join("manifest.json"))?;
    no_reparse(&directory.join("previous.exe"))?;
    let record: Backup = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("上一版本备份信息无效：{e}"))?;
    if record.schema != PROTOCOL
        || !target_name(&record.target)
        || record.abi != plugin_manager::ABI_VERSION
        || !valid_hash(&record.sha256)
        || file_hash(&directory.join("previous.exe"))? != record.sha256
    {
        return Err("上一版本主程序备份校验失败".into());
    }
    Ok(Some((record, directory)))
}
pub fn previous_version() -> Result<Option<String>> {
    let (root, _) = application()?;
    let Some((record, _)) = read_backup(&root)? else {
        return Ok(None);
    };
    if Version::parse(&record.version)?.compare(&Version::parse(env!("CARGO_PKG_VERSION"))?)
        == Ordering::Less
    {
        Ok(Some(record.version))
    } else {
        Ok(None)
    }
}
pub fn prepare_rollback() -> Result<PreparedUpdate> {
    let (root, job, target) = create_job()?;
    let (record, backup) = read_backup(&root)?.ok_or("没有可回退的主程序备份")?;
    if !record.target.eq_ignore_ascii_case(&target)
        || Version::parse(&record.version)?.compare(&Version::parse(env!("CARGO_PKG_VERSION"))?)
            != Ordering::Less
    {
        return Err("备份不是当前主程序的更早版本".into());
    }
    plugin_manager::check_core_compatibility(&record.version)?;
    fs::copy(backup.join("previous.exe"), job.join("candidate.exe")).map_err(|e| e.to_string())?;
    self_check(&job.join("candidate.exe"), &job, &record.version)?;
    finalize_prepare(&root, &job, target, record.version, Vec::new())
}
fn load_job(path: &Path, helper: bool) -> Result<(Job, PathBuf, PathBuf)> {
    if !path.is_absolute() {
        return Err("更新任务必须使用绝对路径".into());
    }
    let path = canonical(path)?;
    if path.file_name().and_then(|n| n.to_str()) != Some("job.json") {
        return Err("更新任务文件名无效".into());
    }
    let directory = path.parent().ok_or("更新任务目录无效")?.to_path_buf();
    let name = directory
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("更新任务目录无效")?;
    if !name.starts_with('u')
        || name.len() > 80
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("更新任务目录标识无效".into());
    }
    let updates = directory.parent().ok_or("更新目录无效")?;
    let data = updates.parent().ok_or("更新目录无效")?;
    let root = data.parent().ok_or("应用目录无效")?.to_path_buf();
    if updates.file_name().and_then(|n| n.to_str()) != Some("updates")
        || data.file_name().and_then(|n| n.to_str()) != Some("data")
    {
        return Err("任务必须位于当前应用 data/updates 目录内".into());
    }
    for p in [data, updates, &directory, &path] {
        no_reparse(p)?;
    }
    let exe = canonical(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    if helper {
        if !same(&exe, &directory.join("helper.exe")) {
            return Err("只能由该任务目录内的更新助手执行".into());
        }
    } else if !same(exe.parent().ok_or("主程序目录无效")?, &root) {
        return Err("任务不属于当前可写应用目录".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 {
        return Err("更新任务过大".into());
    }
    let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if job.schema != PROTOCOL
        || !target_name(&job.target)
        || !valid_hash(&job.old_sha256)
        || !valid_hash(&job.new_sha256)
        || job.documents.len() > 192
    {
        return Err("更新任务内容无效".into());
    }
    Version::parse(&job.old_version)?;
    Version::parse(&job.new_version)?;
    if helper && job.new_version != env!("CARGO_PKG_VERSION") {
        return Err("助手实际版本与更新任务版本不匹配".into());
    }
    if !name.starts_with(&format!("u{}-", job.old_pid)) {
        return Err("任务目录与原进程标识不匹配".into());
    }
    let target = safe_target(&root, Path::new(&job.target))?;
    for filename in ["original.exe", "candidate.exe", "helper.exe"] {
        no_reparse(&directory.join(filename))?;
    }
    if file_hash(&target)? != job.old_sha256
        || file_hash(&directory.join("original.exe"))? != job.old_sha256
        || file_hash(&directory.join("candidate.exe"))? != job.new_sha256
        || file_hash(&directory.join("helper.exe"))? != job.new_sha256
    {
        return Err("更新任务的原件、候选或助手校验失败".into());
    }
    let mut seen = BTreeSet::new();
    for doc in &job.documents {
        let relative = document_path(&doc.path)?;
        safe_target(&directory, &PathBuf::from("documents").join(&relative))?;
        if doc.previous.is_some() {
            safe_target(&directory, &PathBuf::from("old-documents").join(&relative))?;
        }
        if !seen.insert(relative.to_string_lossy().to_ascii_lowercase())
            || !valid_hash(&doc.sha256)
            || file_hash(&directory.join("documents").join(&relative))? != doc.sha256
        {
            return Err("更新文档校验失败".into());
        }
        safe_target(&root, &relative)?;
        if let Some(previous) = &doc.previous {
            if !valid_hash(previous)
                || file_hash(&directory.join("old-documents").join(relative))? != *previous
            {
                return Err("原文档备份校验失败".into());
            }
        }
    }
    Ok((job, root, directory))
}
pub fn launch(update: &PreparedUpdate) -> Result<()> {
    let (job, root, directory) = load_job(&update.job, false)?;
    let (exe, created) = process_identity(std::process::id())?;
    if job.old_pid != std::process::id()
        || job.old_version != env!("CARGO_PKG_VERSION")
        || job.old_created != created
        || !same(&exe, &root.join(&job.target))
    {
        return Err("更新任务不是由当前主程序准备的".into());
    }
    File::options()
        .write(true)
        .create_new(true)
        .open(directory.join("launch.lock"))
        .map_err(|e| format!("更新任务已启动过或无法锁定，请重新准备：{e}"))?;
    if directory.join("ready.json").exists() {
        return Err("更新任务已有旧握手，拒绝重复启动".into());
    }
    let mut process = Command::new(directory.join("helper.exe"))
        .arg("--finish-core-update")
        .arg(&update.job)
        .current_dir(&root)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动更新助手失败：{e}"))?;
    let start = Instant::now();
    let outcome = (|| loop {
        let ready = directory.join("ready.json");
        if ready.is_file() {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(ready).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if value.get("pid").and_then(|v| v.as_u64()) == Some(process.id() as u64)
                && value.get("parent").and_then(|v| v.as_u64()) == Some(std::process::id() as u64)
            {
                return Ok(());
            }
            return Err("更新助手握手不匹配".into());
        }
        if process.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err("更新助手未完成校验即退出，原主程序未关闭".into());
        }
        if start.elapsed() > Duration::from_secs(15) {
            return Err("更新助手准备超时，原主程序未关闭".into());
        }
        thread::sleep(Duration::from_millis(40));
    })();
    if outcome.is_err() {
        let _ = process.kill();
        let _ = process.wait();
    }
    outcome
}
fn promote_backup(root: &Path, job_dir: &Path, job: &Job) -> Result<Option<PathBuf>> {
    let data = ensure_dir(root, "data")?;
    let backups = ensure_dir(&data, "backups")?;
    let key = job_dir.file_name().ok_or("更新目录无效")?.to_string_lossy();
    let pending = backups.join(format!("pending-{key}"));
    fs::create_dir(&pending).map_err(|e| e.to_string())?;
    fs::copy(job_dir.join("original.exe"), pending.join("previous.exe"))
        .map_err(|e| e.to_string())?;
    let record = Backup {
        schema: PROTOCOL,
        target: job.target.clone(),
        version: job.old_version.clone(),
        sha256: job.old_sha256.clone(),
        abi: plugin_manager::ABI_VERSION,
    };
    storage::atomic_write(
        &pending.join("manifest.json"),
        &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
    )?;
    if job_dir.join("config-backup").is_dir() {
        backup_configs(
            &job_dir.join("config-backup"),
            &pending.join("config"),
            false,
            &mut 0,
            &mut 0,
        )?;
    }
    let current = backups.join("current-core");
    let archive = backups.join(format!("saved-{key}"));
    let previous = if current.exists() {
        no_reparse(&current)?;
        fs::rename(&current, &archive).map_err(|e| e.to_string())?;
        Some(archive)
    } else {
        None
    };
    if let Err(error) = fs::rename(&pending, &current) {
        if let Some(previous) = &previous {
            if let Err(restore) = fs::rename(previous, &current) {
                return Err(format!("备份切换失败：{error}；旧备份恢复失败：{restore}"));
            }
        }
        return Err(format!("备份切换失败：{error}"));
    }
    Ok(previous)
}

/// Start a successor which waits for this process to fully exit before opening UI.
/// Used after plugin changes, because the installed device registry is process-local.
pub fn launch_restart() -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(&exe)
        .arg("--restart-after")
        .arg(std::process::id().to_string())
        .current_dir(exe.parent().ok_or("主程序目录无效")?)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动重启进程失败：{e}"))?;
    let mut stdout = child.stdout.take().ok_or("重启握手通道不可用")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        let mut ready = [0u8];
        let result = stdout
            .read_exact(&mut ready)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                if ready == *b"R" {
                    Ok(())
                } else {
                    Err("重启握手不匹配".into())
                }
            });
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("重启准备失败：{e}"))
        .and_then(|ready| ready);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = reader.join();
    result?;
    Ok(())
}
/// Called before loading device plugins or taking the single-instance mutex.
pub fn wait_for_restart_parent(pid: u32) -> Result<()> {
    if parent_pid()? != pid {
        return Err("重启来源进程不匹配".into());
    }
    let (parent_image, _) = process_identity(pid)?;
    let exe = canonical(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    if !same(&parent_image, &exe) {
        return Err("重启来源不是同一主程序".into());
    }
    unsafe {
        let parent = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if parent.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        use std::io::Write;
        if let Err(error) = std::io::stdout()
            .write_all(b"R")
            .and_then(|()| std::io::stdout().flush())
        {
            CloseHandle(parent);
            return Err(format!("重启握手失败：{error}"));
        }
        let wait = WaitForSingleObject(parent, 420000);
        CloseHandle(parent);
        if wait != WAIT_OBJECT_0 {
            return Err("原程序未能及时退出，请手动重新打开".into());
        }
    }
    Ok(())
}

fn restart(root: &Path, target: &str) -> Result<()> {
    Command::new(root.join(target))
        .current_dir(root)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("主程序已替换，但自动重启失败：{e}"))?;
    Ok(())
}
/// Invoked only by main's early --finish-core-update dispatch, before device/UI setup.
pub fn finish_update(path: &Path) -> Result<()> {
    let (job, root, directory) = load_job(path, true)?;
    if parent_pid()? != job.old_pid {
        return Err("更新助手的实际父进程与任务不符".into());
    }
    let (image, created) = process_identity(job.old_pid)?;
    if !same(&image, &root.join(&job.target)) || created != job.old_created {
        return Err("更新助手拒绝其它 PID、映像或已复用的进程".into());
    }
    plugin_manager::check_core_compatibility_at(&root, &job.new_version)?;
    unsafe {
        let parent = OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            job.old_pid,
        );
        if parent.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let ready = storage::atomic_write(
            &directory.join("ready.json"),
            &serde_json::to_vec(
                &serde_json::json!({"pid":std::process::id(),"parent":job.old_pid}),
            )
            .map_err(|e| e.to_string())?,
        );
        if let Err(e) = ready {
            CloseHandle(parent);
            return Err(e);
        }
        let wait = WaitForSingleObject(parent, 420000);
        CloseHandle(parent);
        if wait != WAIT_OBJECT_0 {
            return Err("原主程序未在规定时间退出，更新已取消".into());
        }
    }
    // Revalidate after the old process stopped, before any replacement.
    let checked = load_job(path, true).and_then(|(job, root, directory)| {
        plugin_manager::check_core_compatibility_at(&root, &job.new_version)?;
        Ok((job, root, directory))
    });
    let (job, root, directory) = match checked {
        Ok(value) => value,
        Err(error) => {
            let restore = (|| {
                let original = directory.join("original.exe");
                no_reparse(&original)?;
                if file_hash(&original)? != job.old_sha256 {
                    return Err("原 EXE 备份也未能通过校验".to_string());
                }
                let target = safe_target(&root, Path::new(&job.target))?;
                storage::atomic_write(&target, &fs::read(original).map_err(|e| e.to_string())?)?;
                restart(&root, &job.target)
            })();
            return Err(match restore {
                Ok(()) => format!("更新重新校验失败：{error}；已恢复原 EXE 并重启"),
                Err(restore) => format!("更新重新校验失败：{error}；恢复/重启失败：{restore}"),
            });
        }
    };
    let previous = match promote_backup(&root, &directory, &job) {
        Ok(value) => value,
        Err(error) => {
            let restarted = if file_hash(&root.join(&job.target))? == job.old_sha256 {
                restart(&root, &job.target)
            } else {
                Err("原 EXE 已变化，拒绝执行未验证文件".into())
            };
            return Err(match restarted {
                Ok(()) => format!("更新备份失败：{error}；原程序已重新启动"),
                Err(restart) => format!("更新备份失败：{error}；重启未完成：{restart}"),
            });
        }
    };
    let mut changed = Vec::new();
    let apply = (|| {
        storage::atomic_write(
            &root.join(&job.target),
            &fs::read(directory.join("candidate.exe")).map_err(|e| e.to_string())?,
        )?;
        for doc in &job.documents {
            let relative = document_path(&doc.path)?;
            let target = safe_target(&root, &relative)?;
            storage::atomic_write(
                &target,
                &fs::read(directory.join("documents").join(relative)).map_err(|e| e.to_string())?,
            )?;
            changed.push(doc.clone());
        }
        if file_hash(&root.join(&job.target))? != job.new_sha256 {
            return Err("主程序替换后校验失败".into());
        }
        restart(&root, &job.target)
    })();
    if let Err(error) = apply {
        let restore = (|| {
            storage::atomic_write(
                &root.join(&job.target),
                &fs::read(directory.join("original.exe")).map_err(|e| e.to_string())?,
            )?;
            for doc in changed.iter().rev() {
                let relative = document_path(&doc.path)?;
                let target = safe_target(&root, &relative)?;
                if doc.previous.is_some() {
                    storage::atomic_write(
                        &target,
                        &fs::read(directory.join("old-documents").join(relative))
                            .map_err(|e| e.to_string())?,
                    )?;
                } else if target.exists() && file_hash(&target)? == doc.sha256 {
                    fs::remove_file(&target).map_err(|e| e.to_string())?;
                }
            }
            if let Some(previous) = previous {
                let backups = root.join("data/backups");
                let current = backups.join("current-core");
                let failed = backups.join(format!(
                    "failed-{}",
                    directory.file_name().unwrap().to_string_lossy()
                ));
                fs::rename(&current, failed).map_err(|e| e.to_string())?;
                fs::rename(previous, current).map_err(|e| e.to_string())?;
            }
            restart(&root, &job.target)
        })();
        let message = match restore {
            Ok(()) => format!("主程序更新失败：{error}；已恢复原 EXE 并重启"),
            Err(restore) => format!(
                "主程序更新失败：{error}；恢复/重启未完成：{restore}。备份位于 {}",
                directory.display()
            ),
        };
        let _ = storage::atomic_write(
            &directory.join("result.json"),
            &serde_json::to_vec(&serde_json::json!({"ok":false,"error":message}))
                .unwrap_or_default(),
        );
        return Err(message);
    }
    storage::atomic_write(
        &directory.join("result.json"),
        &serde_json::to_vec(&serde_json::json!({"ok":true,"version":job.new_version}))
            .map_err(|e| e.to_string())?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_versions_and_release_channels() {
        assert_eq!(
            Version::parse("0.5.0-beta.2")
                .unwrap()
                .compare(&Version::parse("0.5.0-beta.10").unwrap()),
            Ordering::Less
        );
        assert_eq!(
            Version::parse("0.5.0")
                .unwrap()
                .compare(&Version::parse("0.5.0-rc.1").unwrap()),
            Ordering::Greater
        );
        assert_eq!(
            Version::parse("0.4.0")
                .unwrap()
                .compare(&Version::parse("0.4.0").unwrap()),
            Ordering::Equal
        );
        assert!(Version::parse("1.0.0-").is_err());
        let releases = || {
            vec![
                Release {
                    tag_name: "v0.4.1".into(),
                    body: String::new(),
                    draft: false,
                    prerelease: false,
                    assets: vec![],
                },
                Release {
                    tag_name: "v0.5.0-beta.1".into(),
                    body: String::new(),
                    draft: false,
                    prerelease: true,
                    assets: vec![],
                },
                Release {
                    tag_name: "v99.0.0".into(),
                    body: String::new(),
                    draft: true,
                    prerelease: false,
                    assets: vec![],
                },
            ]
        };
        assert_eq!(
            choose_release(releases(), "stable").unwrap().1.tag_name,
            "v0.4.1"
        );
        assert_eq!(
            choose_release(releases(), "preview").unwrap().1.tag_name,
            "v0.5.0-beta.1"
        );
    }
    fn info() -> UpdateInfo {
        UpdateInfo {
            version: "0.5.0".into(),
            notes: String::new(),
            url: format!("{REPO}/releases/download/v0.5.0/PeripheralDesk-0.5.0-windows-x64.zip"),
            size: 10,
            sha256: "1".repeat(64),
            available: true,
            channel: "stable".into(),
            tag: "v0.5.0".into(),
        }
    }
    #[test]
    fn only_matching_official_release_assets_and_digest() {
        let mut package = info();
        assert!(trusted_url(&package).is_ok());
        for url in ["https://github.com/chenran0408/peripheral-desk/actions/artifacts/123", "https://github.com/other/peripheral-desk/releases/download/v0.5.0/PeripheralDesk-0.5.0-windows-x64.zip"] {package.url=url.into();assert!(trusted_url(&package).is_err());}
        package = info();
        package.sha256.clear();
        assert!(trusted_url(&package).is_err());
        package = info();
        package.version = "0.6.0".into();
        assert!(trusted_url(&package).is_err());
    }
    #[test]
    fn document_whitelist_excludes_data_paths_and_executables() {
        assert!(document_path("README.md").is_ok());
        for path in [
            "NOTICE",
            "docs/windows-build.yml",
            "THIRD_PARTY_LICENSES/windows/license-apache-2.0",
            "THIRD_PARTY_LICENSES/serde/LICENSE-MIT",
        ] {
            assert!(document_path(path).is_ok(), "{path}");
        }
        assert!(document_path("docs/UPDATING.md").is_ok());
        for path in [
            "data/preferences.json",
            "../PeripheralDesk.exe",
            "C:/outside.md",
            "docs/../../outside.md",
            "docs/note.txt:stream",
            "docs/driver.exe",
        ] {
            assert!(document_path(path).is_err(), "{path}");
        }
    }
    #[test]
    fn zip_data_entries_cannot_touch_existing_user_files() {
        use std::io::Write;
        let root = std::env::var_os("PD_UPDATE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("peripheral-desk-tests"))
            .join(stamp().unwrap());
        fs::create_dir_all(root.join("data")).unwrap();
        fs::create_dir_all(root.join("staging")).unwrap();
        fs::write(root.join("data/unknown.bin"), b"untouched-user-file").unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("data/unknown.bin", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"overwrite-attempt").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(extract(&info(), &bytes, &root.join("staging")).is_err());
        assert_eq!(
            fs::read(root.join("data/unknown.bin")).unwrap(),
            b"untouched-user-file"
        );
        let error = prepare_downloaded(&info(), b"incorrect download").unwrap_err();
        assert!(error.contains("SHA-256"));
        assert_eq!(
            fs::read(root.join("data/unknown.bin")).unwrap(),
            b"untouched-user-file"
        );
    }
}

#[cfg(test)]
mod package_contract_tests {
    use super::*;
    #[test]
    fn standard_distribution_documents_can_be_updated_without_touching_user_data() {
        use std::io::Write;
        fn collect(root: &Path, relative: &Path, files: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(root.join(relative)).unwrap() {
                let entry = entry.unwrap();
                let child = relative.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    collect(root, &child, files)
                } else {
                    files.push((
                        child.to_string_lossy().replace(char::from(92), "/"),
                        fs::read(entry.path()).unwrap(),
                    ));
                }
            }
        }
        let source = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = vec![(
            "PeripheralDesk.exe".into(),
            b"MZ-package-contract-fixture".to_vec(),
        )];
        for name in [
            "README.md",
            "CHANGELOG.md",
            "NOTICE",
            "LICENSE",
            "SOURCES.md",
        ] {
            files.push((name.into(), fs::read(source.join(name)).unwrap()));
        }
        for directory in ["docs", "THIRD_PARTY_LICENSES"] {
            collect(source, Path::new(directory), &mut files);
        }
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in &files {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        let root = std::env::var_os("PD_UPDATE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(stamp().unwrap());
        fs::create_dir_all(root.join("staging")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/unknown.bin"), b"keep").unwrap();
        let info = UpdateInfo {
            version: "0.4.0".into(),
            notes: String::new(),
            url: String::new(),
            size: bytes.len() as u64,
            sha256: hash(&bytes),
            available: true,
            channel: "stable".into(),
            tag: "v0.4.0".into(),
        };
        let documents = extract(&info, &bytes, &root.join("staging")).unwrap();
        assert_eq!(documents.len(), files.len() - 1);
        assert_eq!(fs::read(root.join("data/unknown.bin")).unwrap(), b"keep");
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    #[test]
    fn successor_waits_for_parent_exit() {
        let root = std::env::var_os("PD_UPDATE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("restart-{}", stamp().unwrap()));
        fs::create_dir_all(&root).unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "core_update::restart_tests::parent_probe",
                "--nocapture",
            ])
            .env("PD_RESTART_TEST_DIR", &root)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = parent.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() > deadline {
                let _ = parent.kill();
                panic!("Restart probe timed out");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("completed").is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read(root.join("completed")).unwrap(), b"parent-exited");
    }
    #[test]
    #[ignore = "subprocess fixture invoked by successor_waits_for_parent_exit"]
    // The fixture must exit before its successor; waiting here would deadlock.
    #[allow(clippy::zombie_processes)]
    fn parent_probe() {
        let root = PathBuf::from(std::env::var_os("PD_RESTART_TEST_DIR").unwrap());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "core_update::restart_tests::child_probe",
                "--nocapture",
            ])
            .env("PD_RESTART_PARENT_PID", std::process::id().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        let mut byte = [0u8];
        loop {
            output.read_exact(&mut byte).unwrap();
            if byte == *b"R" {
                break;
            }
        }
        assert!(
            !root.join("completed").exists(),
            "Successor must not initialize before old process exits"
        );
    }
    #[test]
    #[ignore = "subprocess fixture invoked by parent_probe"]
    fn child_probe() {
        let root = PathBuf::from(std::env::var_os("PD_RESTART_TEST_DIR").unwrap());
        let pid = std::env::var("PD_RESTART_PARENT_PID")
            .unwrap()
            .parse()
            .unwrap();
        wait_for_restart_parent(pid).unwrap();
        fs::write(root.join("completed"), b"parent-exited").unwrap();
        std::process::exit(0);
    }
}
