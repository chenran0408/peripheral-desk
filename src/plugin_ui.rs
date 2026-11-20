//! On-demand native driver-package manager. Package verification stays in the library.
use crate::{
    hid::wide,
    paint::{blend, Canvas, BG, BORDER, DIM, FIELD, MUTED, PANEL, R, TEXT},
    ui::widgets::{
        DANGER, HOVER, PRESSED, RADIUS_LG, RADIUS_MD, SPACE_2, SPACE_3, SPACE_4, SUCCESS, WARN,
    },
};
use peripheral_desk::plugin_manager::{self, CatalogBrowse, DriverPackage, InstalledDriver};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::LibraryLoader::*,
    UI::{
        Controls::{Dialogs::*, *},
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};
const WIDTH: i32 = 1040;
const HEIGHT: i32 = 700;
const FINISHED: u32 = WM_APP + 84;
const FOCUS_CHANGED: u32 = WM_APP + 85;
const LIST: i32 = 101;
const REFRESH: i32 = 102;
const INSTALL: i32 = 103;
const REMOVE: i32 = 104;
const DESCRIPTION: i32 = 105;
const STATUS: i32 = 106;
const OFFLINE: i32 = 107;
const CHANNEL: i32 = 108;
const ROLLBACK: i32 = 109;
const CHANGELOG: i32 = 110;
const SELECT_ALL: i32 = 111;
const SELECT_NONE: i32 = 112;
const SELECT_DETECTED: i32 = 113;
const PREVIEW: i32 = 114;
const SELECT_UPDATES: i32 = 115;
const RESTART: i32 = 116;
const BRAND: i32 = 117;
const SEARCH: i32 = 118;
const SEARCH_LABEL: i32 = 119;
const BRAND_LABEL: i32 = 120;
const MHUB: i32 = 121;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreExit {
    Close,
    Restart,
    MHub,
}

#[derive(Default)]
enum Installation {
    #[default]
    Missing,
    Present(String),
    Broken(String),
}
impl Installation {
    fn label(&self) -> &str {
        match self {
            Self::Missing => "未安装",
            Self::Present(version) => version,
            Self::Broken(_) => "安装记录异常",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Recommendation {
    Clear,
    Ambiguous,
}
impl Recommendation {
    fn rank(self) -> u8 {
        match self {
            Self::Clear => 0,
            Self::Ambiguous => 1,
        }
    }
}
struct Inventory {
    packages: Vec<DriverPackage>,
    installed_only: Vec<InstalledDriver>,
    installed: BTreeMap<String, Installation>,
    recommendation: BTreeMap<String, Recommendation>,
    channel: String,
    focus: Option<String>,
}
struct DriverRow<'a> {
    id: &'a str,
    name: &'a str,
    version: &'a str,
    description: &'a str,
    changelog: &'a str,
    device: &'a serde_json::Value,
    browse: Option<&'a CatalogBrowse>,
    capabilities: &'a [String],
    download_bytes: u64,
    catalog_available: bool,
    manual_hint: Option<&'a str>,
    recommendation: Option<Recommendation>,
}
impl DriverRow<'_> {
    fn brand(&self) -> &str {
        self.device["brand"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("未注明厂商")
            .trim()
    }

    fn matches(&self, brand: Option<&str>, terms: &[String]) -> bool {
        if brand.is_some_and(|name| !self.brand().eq_ignore_ascii_case(name)) {
            return false;
        }
        if terms.is_empty() {
            return true;
        }
        let mut searchable = search_key(&format!(
            "{} {} {} {}",
            self.brand(),
            self.name,
            self.id,
            self.device["name"].as_str().unwrap_or_default()
        ));
        if let Some(browse) = self.browse {
            for field in browse.vendor_aliases.iter().chain([&browse.family]) {
                searchable.push(' ');
                searchable.push_str(&search_key(field));
            }
        }
        terms.iter().all(|term| searchable.contains(term.as_str()))
    }

    fn details(&self) -> String {
        let names: Vec<&str> = self
            .capabilities
            .iter()
            .map(|name| match name.as_str() {
                "battery" => "电量",
                "settings" => "设备设置",
                "dpi-stages-read" => "读取 DPI 档位",
                "polling-rate-read" => "配置回报率（只读）",
                "bindings" => "按键映射",
                "lighting-editor" => "灯效编辑",
                "lighting-switch" => "灯光开关",
                "lighting" => "灯光",
                "key-colors" => "逐键灯色",
                "volume" => "音量",
                "effects" => "音效",
                "experimental-onboard-macros" => "板载宏（实验）",
                "software-macro-trigger" => "常驻软件宏",
                other => other,
            })
            .collect();
        let validation = &self.device["validation"];
        let verification = match validation["status"].as_str() {
            Some("hardware-verified") => "本项目已实机验证（限下列范围）",
            Some("upstream-hardware-verified-project-untested") => "上游实机验证，本项目未复测",
            Some("upstream-verified") => "上游已验证",
            Some("code-adapter-only") => "已完成代码适配，实机待验证",
            Some("public-implementation-untested") => "依据公开实现适配，实机待验证",
            _ => "暂未记录",
        };
        let mut lines = vec![self.description.to_owned()];
        if let Some(browse) = self.browse {
            if !browse.vendor_aliases.is_empty() {
                lines.push(format!("厂商别名：{}", browse.vendor_aliases.join("、")));
            }
            let family = browse.family.trim();
            if !family.is_empty() {
                lines.push(format!("协议系列：{family}"));
            }
        }
        if !names.is_empty() {
            lines.push(format!("支持功能：{}", names.join("、")));
        }
        lines.push(format!("验证范围：{verification}"));
        if let Some(details) = validation["project_validation"].as_str() {
            lines.push(details.into());
        }
        let links = &validation["connections"];
        let connection = match links {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join("、"),
            _ => String::new(),
        };
        if !connection.is_empty() {
            lines.push(format!("适用连接：{connection}"));
        }
        lines.push(format!("包标识：{}", self.id));
        lines.join("\r\n")
    }
}

fn search_key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || *c == '+')
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Default)]
struct Browser {
    brands: Vec<String>,
    brand: Option<String>,
    query: String,
    visible: Vec<usize>,
}

impl Browser {
    fn refresh_brands(&mut self, inventory: Option<&Inventory>) {
        let mut names = BTreeMap::new();
        if let Some(items) = inventory {
            for row in (0..items.len()).filter_map(|idx| items.row(idx)) {
                names
                    .entry(row.brand().to_ascii_uppercase())
                    .or_insert_with(|| row.brand().to_owned());
            }
        }
        self.brand = self
            .brand
            .as_ref()
            .and_then(|b| names.get(&b.to_ascii_uppercase()).cloned());
        self.brands = names.into_values().collect();
    }

    fn filter(&mut self, inventory: Option<&Inventory>) {
        self.visible.clear();
        let Some(items) = inventory else { return };
        let terms = self
            .query
            .split_whitespace()
            .map(search_key)
            .collect::<Vec<_>>();
        self.visible.extend((0..items.len()).filter(|index| {
            items
                .row(*index)
                .is_some_and(|row| row.matches(self.brand.as_deref(), &terms))
        }));
    }
}

impl Inventory {
    fn len(&self) -> usize {
        self.packages.len() + self.installed_only.len()
    }
    fn row(&self, index: usize) -> Option<DriverRow<'_>> {
        if let Some(package) = self.packages.get(index) {
            return Some(DriverRow {
                id: &package.id,
                name: &package.name,
                version: &package.version,
                description: &package.description,
                changelog: &package.changelog,
                device: &package.device,
                browse: package.browse.as_ref(),
                capabilities: &package.capabilities,
                download_bytes: package.download_bytes,
                catalog_available: true,
                manual_hint: package
                    .device
                    .get("manual")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                    .then(|| {
                        package
                            .device
                            .get("manual_hint")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("此插件与另一型号共用接收器标识，需要手动确认设备")
                    }),
                recommendation: self.recommendation.get(&package.id).copied(),
            });
        }
        let driver = self
            .installed_only
            .get(index.checked_sub(self.packages.len())?)?;
        Some(DriverRow {
            id: &driver.id,
            name: &driver.manifest.name,
            version: "当前通道无可用更新",
            description: &driver.manifest.description,
            changelog: &driver.manifest.changelog,
            device: &driver.manifest.device,
            browse: None,
            capabilities: &driver.manifest.capabilities,
            download_bytes: 0,
            catalog_available: false,
            manual_hint: driver
                .manifest
                .device
                .get("manual")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                .then(|| {
                    driver
                        .manifest
                        .device
                        .get("manual_hint")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("此插件与另一型号共用接收器标识，需要手动确认设备")
                }),
            recommendation: None,
        })
    }
}
enum Operation {
    Load,
    Refresh,
    Install(Vec<InstallTarget>),
    Remove(String),
    Offline(PathBuf),
    Channel(String),
    Rollback(String),
}
#[derive(Clone)]
struct InstallTarget {
    id: String,
    name: String,
    version: String,
    bytes: u64,
}
struct BatchResult {
    entries: Vec<(InstallTarget, Result<bool, String>)>,
}
impl BatchResult {
    fn failed(&self) -> bool {
        self.entries.iter().any(|(_, result)| result.is_err())
    }
    fn description(&self) -> String {
        let installed = self
            .entries
            .iter()
            .filter(|(_, r)| matches!(r, Ok(true)))
            .count();
        let skipped = self
            .entries
            .iter()
            .filter(|(_, r)| matches!(r, Ok(false)))
            .count();
        let failed = self.entries.len() - installed - skipped;
        let mut lines = vec![format!(
            "安装完成：成功 {installed} 项，失败 {failed} 项，跳过已同版 {skipped} 项。全部成功后将自动重启生效。"
        )];
        for (target, result) in &self.entries {
            lines.push(match result {
                Ok(true) => format!("成功 · {} {}", target.name, target.version),
                Ok(false) => format!("跳过 · {} {} 已安装同版本", target.name, target.version),
                Err(error) => format!("失败 · {}：{error}", target.name),
            });
        }
        lines.join("\r\n")
    }
}
fn install_batch(
    targets: Vec<InstallTarget>,
    mut current: impl FnMut(&str) -> Option<String>,
    mut install: impl FnMut(&str) -> Result<(), String>,
    mut progress: impl FnMut(usize, usize, &InstallTarget),
) -> BatchResult {
    // Each item enters the normal verified installer independently. Never let
    // one failed download prevent the remaining explicitly selected items.
    let count = targets.len();
    BatchResult {
        entries: targets
            .into_iter()
            .enumerate()
            .map(|(index, target)| {
                progress(index + 1, count, &target);
                let result = if current(&target.id).as_deref() == Some(&target.version) {
                    Ok(false)
                } else {
                    install(&target.id).map(|()| true)
                };
                (target, result)
            })
            .collect(),
    }
}
struct WorkResult {
    changed: bool,
    inventory: Result<Inventory, String>,
    batch: Option<BatchResult>,
}
enum WorkEvent {
    RestartReady(Result<(), String>),
    Progress(String),
    Finished(WorkResult),
}
struct Child {
    hwnd: HWND,
    id: i32,
    label: String,
}
struct Manager {
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    children: Vec<Child>,
    inventory: Option<Inventory>,
    selected: usize,
    browser: Browser,
    checked: BTreeSet<String>,
    pending: Option<Receiver<WorkEvent>>,
    status: String,
    success: String,
    error: bool,
    dialog_open: bool,
    changing_channel: bool,
    select_updates_after_refresh: bool,
    changed: bool,
    exit: StoreExit,
}
impl Drop for Manager {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.brush);
        }
    }
}
fn colorref(argb: u32) -> u32 {
    ((argb >> 16) & 255) | (argb & 0xff00) | ((argb & 255) << 16)
}
fn size(bytes: u64) -> String {
    if bytes == 0 {
        "大小未提供".into()
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}
/// Splits detected hardware into unambiguous recommendations and ambiguous
/// candidates. Manual-claim packages share a receiver identity with another
/// model, so hardware presence must never select them implicitly.
fn classify_detection(
    packages: &[plugin_manager::DriverPackage],
    interfaces: &[crate::hid::Interface],
) -> BTreeMap<String, Recommendation> {
    let manifests: Vec<Option<crate::devices::DeviceManifest>> = packages
        .iter()
        .map(|p| serde_json::from_value(p.device.clone()).ok())
        .collect();
    let mut recommendation = BTreeMap::new();
    for interface in interfaces {
        let candidates: Vec<_> = manifests
            .iter()
            .zip(packages)
            .filter_map(|(manifest, package)| {
                let m = manifest.as_ref()?;
                m.matches
                    .iter()
                    .any(|rule| rule.matches(interface))
                    .then_some((m, package))
            })
            .collect();
        for (manifest, package) in &candidates {
            let entry = recommendation
                .entry(package.id.clone())
                .or_insert(Recommendation::Ambiguous);
            if candidates.len() == 1
                && !manifest.manual
                && package.device["runtime_identity"].as_bool() != Some(true)
            {
                *entry = Recommendation::Clear;
            }
        }
    }
    recommendation
}
fn inventory(refresh: bool) -> Result<Inventory, String> {
    let mut packages = if refresh {
        plugin_manager::refresh_catalog()
    } else {
        plugin_manager::catalog()
    }
    .map_err(|e| e.to_string())?;
    let interfaces = crate::hid::enumerate()?;
    let recommendation = classify_detection(&packages, &interfaces);
    packages.sort_by_key(|p| recommendation.get(&p.id).map_or(2, |r| r.rank()));
    let mut installed = BTreeMap::new();
    for package in &packages {
        let status = match plugin_manager::installed(&package.id) {
            Ok(Some(driver)) if plugin_manager::is_installed(&package.id) => {
                Installation::Present(driver.version)
            }
            Ok(_) => Installation::Missing,
            Err(error) => Installation::Broken(error.to_string()),
        };
        installed.insert(package.id.clone(), status);
    }
    // One corrupt installation must not hide unrelated usable packages.
    // Catalog entries already preserve their precise Broken(error) above.
    let installed_only = plugin_manager::installed_descriptors()
        .into_iter()
        .filter_map(|descriptor| {
            descriptor
                .get("key")
                .and_then(|key| key.as_str())
                .map(str::to_owned)
        })
        .filter(|id| !packages.iter().any(|package| package.id == *id))
        .filter_map(|id| plugin_manager::installed(&id).ok().flatten())
        .collect::<Vec<_>>();
    for driver in &installed_only {
        installed.insert(
            driver.id.clone(),
            Installation::Present(driver.version.clone()),
        );
    }
    Ok(Inventory {
        packages,
        installed_only,
        recommendation,
        installed,
        channel: plugin_manager::channel(),
        focus: None,
    })
}
fn updatable_ids(inventory: &Inventory) -> BTreeSet<String> {
    inventory
        .packages
        .iter()
        .filter_map(|package| match inventory.installed.get(&package.id) {
            Some(Installation::Present(current))
                if peripheral_desk::core_update::is_newer_version(&package.version, current)
                    .unwrap_or(false) =>
            {
                Some(package.id.clone())
            }
            _ => None,
        })
        .collect()
}

impl Manager {
    fn px(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / 96) as i32
    }
    fn child(&self, id: i32) -> HWND {
        self.children
            .iter()
            .find(|c| c.id == id)
            .map_or(null_mut(), |c| c.hwnd)
    }
    fn selected(&self) -> Option<DriverRow<'_>> {
        self.inventory.as_ref()?.row(self.selected)
    }
    fn offers_mhub(&self) -> bool {
        self.browser
            .brand
            .as_ref()
            .is_some_and(|b| b.eq_ignore_ascii_case("MCHOSE"))
            || self
                .selected()
                .is_some_and(|p| p.brand().eq_ignore_ascii_case("MCHOSE"))
    }
    fn installation(&self) -> &Installation {
        static MISSING: Installation = Installation::Missing;
        self.selected()
            .and_then(|p| self.inventory.as_ref()?.installed.get(p.id))
            .unwrap_or(&MISSING)
    }
    fn targets(&self) -> Vec<InstallTarget> {
        self.inventory
            .iter()
            .flat_map(|i| &i.packages)
            .filter(|p| self.checked.contains(&p.id))
            .map(|p| InstallTarget {
                id: p.id.clone(),
                name: p.name.clone(),
                version: p.version.clone(),
                bytes: p.download_bytes,
            })
            .collect()
    }
    unsafe fn add(&mut self, hwnd: HWND, id: i32, class: &str, label: &str, r: R, style: u32) {
        let child = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD
                | if class == "STATIC" { 0 } else { WS_TABSTOP }
                | crate::paint::native_control_style(class, style),
            self.px(r.x),
            self.px(r.y),
            self.px(r.w),
            self.px(r.h),
            hwnd,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(child, WM_SETFONT, self.font as usize, 0);
        if matches!(class, "EDIT" | "LISTBOX" | "COMBOBOX") {
            crate::paint::theme_native_control(child, class, self.dpi);
        }
        if id == LIST {
            SetWindowSubclass(child, Some(list_proc), 1, 0);
        }
        self.children.push(Child {
            hwnd: child,
            id,
            label: label.into(),
        });
    }
    unsafe fn text(&self, id: i32, text: &str) {
        SetWindowTextW(self.child(id), wide(text).as_ptr());
    }
    unsafe fn layout(&mut self, hwnd: HWND) {
        for child in self.children.drain(..) {
            DestroyWindow(child.hwnd);
        }
        let channel_x = 486;
        for (id, label, x) in [
            (CHANNEL, "稳定版", channel_x),
            (PREVIEW, "预览版", channel_x + 76 + SPACE_2),
        ] {
            self.add(
                hwnd,
                id,
                "BUTTON",
                label,
                R::new(x, 24, 76, 32),
                BS_OWNERDRAW as u32,
            );
        }
        self.add(
            hwnd,
            OFFLINE,
            "BUTTON",
            "离线安装…",
            R::new(channel_x + 76 * 2 + SPACE_2 + SPACE_4, 24, 136, 32),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            REFRESH,
            "BUTTON",
            "检查更新",
            R::new(
                channel_x + 76 * 2 + SPACE_2 + SPACE_4 + 136 + SPACE_3,
                24,
                176,
                32,
            ),
            BS_OWNERDRAW as u32,
        );
        self.browser.refresh_brands(self.inventory.as_ref());
        self.add(
            hwnd,
            BRAND_LABEL,
            "STATIC",
            "厂商(&B)",
            R::new(44, 107, 76, 20),
            0,
        );
        self.add(
            hwnd,
            BRAND,
            "COMBOBOX",
            "",
            R::new(44, 132, 190, 300),
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        );
        let brands = self.child(BRAND);
        SendMessageW(brands, CB_ADDSTRING, 0, wide("全部厂商").as_ptr() as isize);
        for name in &self.browser.brands {
            SendMessageW(brands, CB_ADDSTRING, 0, wide(name).as_ptr() as isize);
        }
        let choice = self
            .browser
            .brands
            .iter()
            .position(|b| Some(b) == self.browser.brand.as_ref())
            .map_or(0, |n| n + 1);
        SendMessageW(brands, CB_SETCURSEL, choice, 0);
        self.add(
            hwnd,
            SEARCH_LABEL,
            "STATIC",
            "搜索(&F)",
            R::new(250, 107, 160, 20),
            0,
        );
        self.add(
            hwnd,
            SEARCH,
            "EDIT",
            &self.browser.query.clone(),
            R::new(250, 132, 335, 30),
            ES_AUTOHSCROLL as u32,
        );
        SendMessageW(
            self.child(SEARCH),
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            (self.px(8) * 0x10001) as isize,
        );
        SendMessageW(
            self.child(SEARCH),
            EM_SETCUEBANNER,
            1,
            wide("输入型号、厂商或协议系列").as_ptr() as isize,
        );
        self.add(
            hwnd,
            LIST,
            "LISTBOX",
            "",
            R::new(40, 238, 555, 264),
            LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32
                | LBS_OWNERDRAWFIXED as u32
                | LBS_HASSTRINGS as u32
                | LBS_MULTIPLESEL as u32
                | WS_VSCROLL,
        );
        SendMessageW(self.child(LIST), LB_SETITEMHEIGHT, 0, self.px(40) as isize);
        self.populate_list();
        for (id, label, x, width) in [
            (SELECT_ALL, "选中筛选结果", 44, 126),
            (SELECT_NONE, "清空已选", 178, 100),
            (SELECT_DETECTED, "选择已识别", 286, 126),
            (SELECT_UPDATES, "选择可更新", 420, 126),
        ] {
            self.add(
                hwnd,
                id,
                "BUTTON",
                label,
                R::new(x, 176, width, 30),
                BS_OWNERDRAW as u32,
            );
        }

        self.add(
            hwnd,
            MHUB,
            "BUTTON",
            "打开驱动中心",
            R::new(862, 168, 132, 30),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            DESCRIPTION,
            "EDIT",
            "",
            R::new(634, 211, 356, 282),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        self.add(
            hwnd,
            INSTALL,
            "BUTTON",
            "安装所选驱动",
            R::new(44, 532, 208, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            REMOVE,
            "BUTTON",
            "卸载焦点项 · 保留配置",
            R::new(268, 532, 208, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            ROLLBACK,
            "BUTTON",
            "回退焦点项",
            R::new(492, 532, 176, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            CHANGELOG,
            "BUTTON",
            "查看更新内容",
            R::new(684, 532, 176, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            RESTART,
            "BUTTON",
            "重启生效",
            R::new(876, 532, 120, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            STATUS,
            "EDIT",
            &self.status.clone(),
            R::new(28, 636, 984, 46),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        self.selection(hwnd);
        for child in &self.children {
            ShowWindow(child.hwnd, SW_SHOWNA);
        }
        self.availability();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn populate_list(&mut self) {
        let list = self.child(LIST);
        self.browser.filter(self.inventory.as_ref());
        SendMessageW(list, WM_SETREDRAW, 0, 0);
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        if let Some(items) = &self.inventory {
            for (position, &index) in self.browser.visible.iter().enumerate() {
                let row = items.row(index).unwrap();
                SendMessageW(list, LB_ADDSTRING, 0, wide(row.name).as_ptr() as isize);
                SendMessageW(
                    list,
                    LB_SETSEL,
                    usize::from(self.checked.contains(row.id)),
                    position as isize,
                );
            }
        }
        if !self.browser.visible.is_empty() {
            let position = self
                .browser
                .visible
                .iter()
                .position(|&n| n == self.selected)
                .unwrap_or(0);
            self.selected = self.browser.visible[position];
            SendMessageW(list, LB_SETCARETINDEX, position, 0);
        } else {
            self.selected = usize::MAX;
            let message = if self.inventory.is_some()
                && (self.browser.brand.is_some() || !self.browser.query.is_empty())
            {
                "无匹配型号，试试其他厂商或搜索词。"
            } else {
                "暂无可用驱动，检查更新或切换通道。"
            };
            SendMessageW(list, LB_ADDSTRING, 0, wide(message).as_ptr() as isize);
        }
        SendMessageW(list, WM_SETREDRAW, 1, 0);
    }

    unsafe fn sync_caret(&mut self) {
        let position = SendMessageW(self.child(LIST), LB_GETCARETINDEX, 0, 0);
        if let Some(&index) = self.browser.visible.get(position as usize) {
            self.selected = index;
        }
    }

    unsafe fn selection(&mut self, hwnd: HWND) {
        let description = self
            .selected()
            .map(|p| p.details())
            .unwrap_or_else(|| "选择设备型号可查看连接方式、支持功能与验证范围。".into());
        let selected = self.selected();
        let description = match (
            selected.as_ref().and_then(|p| p.manual_hint),
            selected.as_ref().and_then(|p| p.recommendation),
        ) {
            (Some(hint), _) => {
                let claimed = self
                    .selected()
                    .is_some_and(|p| crate::plugin_manager::is_claimed(p.id));
                format!(
                    "{description}\r\n共用接收器型号 · 手动认领：{hint}\r\n认领状态：{}",
                    if claimed {
                        "已认领"
                    } else {
                        "安装本插件即视为确认型号并认领"
                    }
                )
            }
            (None, Some(Recommendation::Ambiguous)) => {
                format!("{description}\r\n仅凭当前接口无法确定型号，请先核对设备再安装。")
            }
            _ => description,
        };
        let detail = match self.installation() {
            Installation::Broken(error) => format!("{description}\r\n安装记录异常：{error}"),
            _ => description,
        };
        self.text(DESCRIPTION, &detail);
        let count = self.targets().len();
        let label = if count == 0 {
            "请勾选设备驱动".into()
        } else {
            format!("安装 / 更新已选 {count} 项")
        };
        if let Some(child) = self.children.iter_mut().find(|c| c.id == INSTALL) {
            child.label = label.clone();
        }
        self.text(INSTALL, &label);
        self.availability();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn availability(&self) {
        let ready = self.pending.is_none() && !self.dialog_open;
        let hub = self.child(MHUB);
        let offers_mhub = self.offers_mhub();
        ShowWindow(hub, if offers_mhub { SW_SHOWNA } else { SW_HIDE });
        EnableWindow(hub, (ready && offers_mhub) as i32);
        EnableWindow(self.child(RESTART), (ready && self.changed) as i32);
        EnableWindow(self.child(REFRESH), ready as i32);
        EnableWindow(self.child(LIST), ready as i32);
        for id in [SEARCH, BRAND] {
            EnableWindow(self.child(id), (ready && self.inventory.is_some()) as i32);
        }
        EnableWindow(self.child(OFFLINE), ready as i32);
        EnableWindow(self.child(CHANNEL), ready as i32);
        EnableWindow(self.child(PREVIEW), ready as i32);
        for id in [SELECT_ALL, SELECT_NONE, SELECT_DETECTED, SELECT_UPDATES] {
            EnableWindow(self.child(id), (ready && self.inventory.is_some()) as i32);
        }
        EnableWindow(
            self.child(CHANGELOG),
            (ready && self.selected().is_some()) as i32,
        );
        EnableWindow(
            self.child(ROLLBACK),
            (ready && matches!(self.installation(), Installation::Present(_))) as i32,
        );
        EnableWindow(
            self.child(INSTALL),
            (ready && !self.targets().is_empty()) as i32,
        );
        EnableWindow(
            self.child(REMOVE),
            (ready
                && self.selected().is_some()
                && !matches!(self.installation(), Installation::Missing)) as i32,
        );
    }
    unsafe fn status(&mut self, hwnd: HWND, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.error = error;
        self.text(STATUS, &self.status);
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn begin(&mut self, hwnd: HWND, operation: Operation) {
        if self.pending.is_some() || self.dialog_open {
            return;
        }
        self.changing_channel = matches!(operation, Operation::Channel(_));
        if self.changing_channel {
            self.checked.clear();
        }
        let (progress, success) = match &operation {
            Operation::Load => (
                "正在读取驱动目录…",
                "勾选所需型号后点击安装；无需按 Ctrl，可一次选择多个驱动。",
            ),
            Operation::Refresh => ("正在检查驱动更新并刷新目录…", "驱动更新检查完成。"),
            Operation::Install(_) => (
                "正在逐项下载并校验已选驱动，请等待完成…",
                "批量安装结束，请查看各项结果。全部成功后将自动重启生效。",
            ),
            Operation::Remove(_) => (
                "正在卸载所选驱动，保留用户配置…",
                "所选驱动已卸载，配置已保留。完成后将自动重启生效。",
            ),
            Operation::Offline(_) => (
                "正在校验离线驱动包并安装…",
                "离线驱动已安装。已有设备关闭窗口后刷新；全部成功后将自动重启生效。",
            ),
            Operation::Channel(_) => (
                "正在切换通道并刷新驱动仓库…",
                "更新通道已切换，驱动目录已刷新。",
            ),
            Operation::Rollback(_) => (
                "正在恢复所选驱动的上一版本…",
                "驱动已回退。完成后将自动重启生效。",
            ),
        };
        self.success = success.into();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status(hwnd, progress, false);
        self.availability();
        let window = hwnd as usize;
        thread::spawn(move || {
            let mut batch = None;
            let mut changed = false;
            let result = (|| match operation {
                Operation::Load => inventory(false),
                Operation::Refresh => inventory(true),
                Operation::Install(targets) => {
                    batch = Some(install_batch(
                        targets,
                        plugin_manager::verified_installed_version,
                        |id| {
                            plugin_manager::install(id)
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        },
                        |index, count, target| {
                            let _ = tx.send(WorkEvent::Progress(format!(
                                "正在处理 {index}/{count}：{} {}（下载、校验与备份）…",
                                target.name, target.version
                            )));
                            unsafe {
                                PostMessageW(window as HWND, FINISHED, 0, 0);
                            }
                        },
                    ));
                    changed = batch
                        .as_ref()
                        .is_some_and(|b| b.entries.iter().any(|(_, r)| matches!(r, Ok(true))));
                    inventory(false)
                }
                Operation::Offline(path) => {
                    let installed =
                        plugin_manager::install_offline(&path).map_err(|e| e.to_string())?;
                    changed = true;
                    let mut state = inventory(false)?;
                    state.focus = Some(installed.id);
                    Ok(state)
                }
                Operation::Rollback(id) => {
                    let installed = plugin_manager::rollback(&id).map_err(|e| e.to_string())?;
                    changed = true;
                    let mut state = inventory(false)?;
                    state.focus = Some(installed.id);
                    Ok(state)
                }
                Operation::Channel(channel) => {
                    let previous = plugin_manager::channel();
                    plugin_manager::set_channel(&channel).map_err(|e| e.to_string())?;
                    match inventory(true) {
                        Ok(state) => Ok(state),
                        Err(error) => match plugin_manager::set_channel(&previous) {
                            Ok(()) => Err(format!("{error}；已恢复原通道与目录")),
                            Err(restore) => Err(format!("{error}；原通道恢复失败：{restore}")),
                        },
                    }
                }
                Operation::Remove(id) => {
                    plugin_manager::uninstall(&id).map_err(|e| e.to_string())?;
                    changed = true;
                    inventory(false)
                }
            })();
            if tx
                .send(WorkEvent::Finished(WorkResult {
                    inventory: result,
                    batch,
                    changed,
                }))
                .is_ok()
            {
                unsafe {
                    PostMessageW(window as HWND, FINISHED, 0, 0);
                }
            }
        });
    }
    unsafe fn finish(&mut self, hwnd: HWND) {
        let Some(receiver) = self.pending.as_ref() else {
            return;
        };
        let event = match receiver.try_recv() {
            Ok(WorkEvent::RestartReady(result)) => {
                self.pending = None;
                match result {
                    Ok(()) => {
                        self.exit = StoreExit::Restart;
                        DestroyWindow(hwnd);
                    }
                    Err(error) => {
                        self.status(
                            hwnd,
                            format!("插件已保存；{error}。可点击“重启生效”重试。"),
                            true,
                        );
                        self.availability();
                    }
                }
                return;
            }
            Ok(WorkEvent::Progress(progress)) => {
                self.status(hwnd, progress, false);
                return;
            }
            Ok(WorkEvent::Finished(result)) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(error) => WorkResult {
                inventory: Err(format!("后台任务未返回结果：{error}")),
                batch: None,
                changed: false,
            },
        };
        self.pending = None;
        self.changed |= event.changed;
        let auto_restart = event.changed
            && event.inventory.is_ok()
            && !event.batch.as_ref().is_some_and(BatchResult::failed);
        let selected = self.selected().map(|p| p.id.to_string());
        let report = event.batch.as_ref().map(BatchResult::description);
        let failed = event.batch.as_ref().is_some_and(BatchResult::failed);
        match event.inventory {
            Ok(inventory) => {
                self.selected = inventory
                    .focus
                    .clone()
                    .or(selected)
                    .and_then(|id| {
                        (0..inventory.len())
                            .position(|index| inventory.row(index).is_some_and(|p| p.id == id))
                    })
                    .unwrap_or(0);
                self.inventory = Some(inventory);
                if self.select_updates_after_refresh {
                    self.checked = self
                        .inventory
                        .as_ref()
                        .map(updatable_ids)
                        .unwrap_or_default();
                    self.select_updates_after_refresh = false;
                    self.success = format!(
                        "驱动更新检查完成；已选择 {} 项可更新驱动。",
                        self.checked.len()
                    );
                }
                if let Some(inventory) = &self.inventory {
                    self.checked
                        .retain(|id| inventory.packages.iter().any(|p| p.id == *id));
                }
                self.status = report.unwrap_or_else(|| self.success.clone());
                self.error = failed;
                self.layout(hwnd);
            }
            Err(error) => {
                let message = if let Some(report) = report {
                    format!("{report}\r\n目录读取失败（已完成的安装不会撤销）：{error}")
                } else {
                    format!("操作未完成：{error}")
                };
                self.status(hwnd, message, true);
                if self.changing_channel {
                    if self
                        .inventory
                        .as_ref()
                        .is_some_and(|i| i.channel != plugin_manager::channel())
                    {
                        self.inventory = None;
                    }
                    self.layout(hwnd);
                } else {
                    self.availability();
                }
            }
        }
        if auto_restart {
            self.restart(hwnd);
        }
    }
    unsafe fn restart(&mut self, hwnd: HWND) {
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status(hwnd, "正在准备重启，设备配置已保存…", false);
        self.availability();
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = peripheral_desk::core_update::launch_restart();
            if tx.send(WorkEvent::RestartReady(result)).is_ok() {
                unsafe {
                    PostMessageW(window as HWND, FINISHED, 0, 0);
                }
            }
        });
    }
    unsafe fn offline(&mut self, hwnd: HWND) {
        if self.pending.is_some() || self.dialog_open {
            return;
        }
        let mut file = [0u16; 32768];
        let filter = wide("外设盒 驱动包 (*.zip)\0*.zip\0\0");
        let title = wide("选择离线驱动包（必须匹配已缓存的受信驱动目录）");
        let mut dialog: OPENFILENAMEW = zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = hwnd;
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        self.dialog_open = true;
        let accepted = GetOpenFileNameW(&mut dialog);
        self.dialog_open = false;
        if accepted != 0 {
            let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
            let path = PathBuf::from(String::from_utf16_lossy(&file[..len]));
            self.begin(hwnd, Operation::Offline(path));
        } else {
            let error = CommDlgExtendedError();
            if error != 0 {
                self.status(hwnd, format!("选择离线驱动包失败：{error}"), true);
            }
        }
    }
    unsafe fn command(&mut self, hwnd: HWND, id: i32, event: u32) {
        if self.pending.is_some() || self.dialog_open {
            return;
        }
        if matches!(id, REMOVE | ROLLBACK | CHANGELOG) {
            self.sync_caret();
        }
        match id {
            SEARCH if event == EN_CHANGE => {
                let edit = self.child(SEARCH);
                if edit.is_null() || self.child(LIST).is_null() {
                    return;
                }
                let mut buffer = vec![0; GetWindowTextLengthW(edit) as usize + 1];
                let read = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32);
                self.browser.query = String::from_utf16_lossy(&buffer[..read as usize]);
                self.populate_list();
                self.selection(hwnd);
            }
            BRAND if event == CBN_SELCHANGE => {
                let choice = SendMessageW(self.child(BRAND), CB_GETCURSEL, 0, 0);
                self.browser.brand = usize::try_from(choice - 1)
                    .ok()
                    .and_then(|n| self.browser.brands.get(n))
                    .cloned();
                self.populate_list();
                self.selection(hwnd);
            }
            LIST if event == LBN_SELCHANGE => {
                self.sync_caret();
                let list = self.child(LIST);
                if let Some(inventory) = &self.inventory {
                    for (position, &index) in self.browser.visible.iter().enumerate() {
                        let row = inventory.row(index).unwrap();
                        if row.catalog_available && SendMessageW(list, LB_GETSEL, position, 0) > 0 {
                            self.checked.insert(row.id.to_owned());
                        } else {
                            self.checked.remove(row.id);
                            if !row.catalog_available {
                                SendMessageW(list, LB_SETSEL, 0, position as isize);
                            }
                        }
                    }
                }
                self.selection(hwnd);
            }
            SELECT_ALL | SELECT_NONE | SELECT_DETECTED | SELECT_UPDATES => {
                if id == SELECT_NONE {
                    self.checked.clear();
                } else if let Some(inventory) = &self.inventory {
                    let updates = updatable_ids(inventory);
                    for &index in &self.browser.visible {
                        let row = inventory.row(index).unwrap();
                        if row.catalog_available
                            && (id == SELECT_ALL
                                || id == SELECT_DETECTED
                                    && row.recommendation == Some(Recommendation::Clear)
                                || id == SELECT_UPDATES && updates.contains(row.id))
                        {
                            self.checked.insert(row.id.to_owned());
                        }
                    }
                }
                self.populate_list();
                self.selection(hwnd);
            }
            MHUB if self.offers_mhub() => {
                self.exit = StoreExit::MHub;
                DestroyWindow(hwnd);
            }
            RESTART if self.changed => self.restart(hwnd),
            REFRESH => self.begin(hwnd, Operation::Refresh),
            OFFLINE => self.offline(hwnd),
            CHANNEL | PREVIEW => {
                let channel = if id == PREVIEW { "preview" } else { "stable" };
                if self
                    .inventory
                    .as_ref()
                    .is_some_and(|i| i.channel == channel)
                {
                    return;
                }
                self.begin(hwnd, Operation::Channel(channel.into()));
            }
            ROLLBACK => {
                if let Some(package) = self.selected() {
                    self.begin(hwnd, Operation::Rollback(package.id.to_string()));
                }
            }
            CHANGELOG => {
                if let Some(package) = self.selected() {
                    let content = if package.changelog.trim().is_empty() {
                        "仓库暂未提供该版本的更新说明。"
                    } else {
                        package.changelog
                    };
                    self.text(
                        DESCRIPTION,
                        &format!(
                            "{} · {} 更新内容\r\n{}",
                            package.name,
                            if package.catalog_available {
                                package.version
                            } else {
                                self.installation().label()
                            },
                            content
                        ),
                    );
                    SetFocus(self.child(DESCRIPTION));
                }
            }
            INSTALL => {
                let targets = self.targets();
                if !targets.is_empty() {
                    self.begin(hwnd, Operation::Install(targets));
                }
            }
            REMOVE => {
                if let Some(package) = self.selected() {
                    self.begin(hwnd, Operation::Remove(package.id.to_string()));
                }
            }
            IDCANCEL => {
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            _ => {}
        }
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = zeroed();
        let dest = BeginPaint(hwnd, &mut ps);
        let mut area = zeroed();
        GetClientRect(hwnd, &mut area);
        let dc = CreateCompatibleDC(dest);
        let bitmap = CreateCompatibleBitmap(dest, area.right.max(1), area.bottom.max(1));
        let old = SelectObject(dc, bitmap);
        {
            let c = Canvas::new(dc, self.dpi);
            c.rect(R::new(0, 0, WIDTH, HEIGHT), BG);
            c.gradient_v(R::new(0, 0, WIDTH, 84), blend(BG, 0xff25131a, 0.8), BG);
            c.text("驱动商店", R::new(28, 13, 350, 32), 22, 600, TEXT, DT_LEFT);
            c.text(
                "查找型号，按需安装设备插件",
                R::new(28, 48, 360, 22),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "更新通道",
                R::new(404, 28, 74, 24),
                12,
                500,
                MUTED,
                DT_RIGHT,
            );
            c.round(R::new(24, 88, 584, 428), RADIUS_LG, PANEL, Some(BORDER));
            c.text(
                &format!("设备型号  ·  {} 个结果", self.browser.visible.len()),
                R::new(48, 211, 280, 23),
                13,
                500,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "安装状态",
                R::new(431, 211, 145, 23),
                12,
                400,
                MUTED,
                DT_RIGHT,
            );
            c.round(R::new(618, 88, 398, 428), RADIUS_LG, PANEL, Some(BORDER));
            c.round(R::new(628, 203, 370, 299), RADIUS_MD, FIELD, None);
            if let Some(row) = self.selected() {
                let stamp = match row.recommendation {
                    Some(Recommendation::Clear) => ("已识别", SUCCESS),
                    Some(Recommendation::Ambiguous) => ("需核对型号", WARN),
                    None if !row.catalog_available => ("仅本机已安装", MUTED),
                    None => ("目录中的型号", MUTED),
                };
                c.text(
                    row.brand(),
                    R::new(634, 103, 210, 22),
                    12,
                    500,
                    MUTED,
                    DT_LEFT,
                );
                c.text(
                    stamp.0,
                    R::new(846, 103, 148, 22),
                    12,
                    500,
                    stamp.1,
                    DT_RIGHT,
                );
                c.text(
                    row.name,
                    R::new(634, 133, 362, 32),
                    19,
                    600,
                    TEXT,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                let version = if row.catalog_available {
                    format!("{}  ·  {}", row.version, size(row.download_bytes))
                } else {
                    self.installation().label().to_owned()
                };
                c.text(
                    &version,
                    R::new(634, 173, if self.offers_mhub() { 220 } else { 362 }, 22),
                    13,
                    400,
                    MUTED,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
            } else {
                c.text(
                    "型号详情",
                    R::new(634, 104, 362, 32),
                    19,
                    600,
                    TEXT,
                    DT_LEFT,
                );
            }
            let targets = self.targets();
            let bytes: u64 = targets.iter().map(|t| t.bytes).sum();
            let visible_checked = self.inventory.as_ref().map_or(0, |inv| {
                self.browser
                    .visible
                    .iter()
                    .filter(|&&i| inv.row(i).is_some_and(|row| self.checked.contains(row.id)))
                    .count()
            });
            let hidden = targets.len().saturating_sub(visible_checked);
            let summary = if hidden > 0 {
                format!(
                    "已选 {} 项，其中 {hidden} 项在其他筛选结果中  ·  下载 {}",
                    targets.len(),
                    size(bytes)
                )
            } else if !targets.is_empty() {
                format!("已选 {} 项  ·  下载 {}", targets.len(), size(bytes))
            } else {
                "点击列表行或按空格勾选型号".into()
            };
            c.text(&summary, R::new(28, 571, 984, 18), 12, 400, MUTED, DT_LEFT);
            c.text(
                "安装、更新、卸载或回退成功后自动重启；部分失败时先保留结果，可点击“重启生效”。",
                R::new(28, 590, 984, 22),
                13,
                400,
                DIM,
                DT_LEFT,
            );
            c.text(
                if self.pending.is_some() {
                    "后台操作中 · 请等待完成后关闭"
                } else if self.error {
                    "操作结果 · 下方可滚动查看具体原因"
                } else {
                    "操作结果 · 可滚动查看每项详情"
                },
                R::new(28, 612, 984, 21),
                12,
                400,
                if self.error { DANGER } else { DIM },
                DT_LEFT,
            );
            c.round(R::new(24, 634, 992, 52), RADIUS_MD, FIELD, Some(BORDER));
        }
        BitBlt(dest, 0, 0, area.right, area.bottom, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
    unsafe fn button(&self, item: &DRAWITEMSTRUCT) {
        let Some(child) = self.children.iter().find(|c| c.hwnd == item.hwndItem) else {
            return;
        };
        // Clear every physical pixel, including antialiased rounded corners.
        let background = if matches!(child.id, REFRESH | OFFLINE | CHANNEL | PREVIEW) {
            BG
        } else {
            PANEL
        };
        let brush = CreateSolidBrush(
            ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
        );
        FillRect(item.hDC, &item.rcItem, brush);
        DeleteObject(brush);
        let c = Canvas::new(item.hDC, self.dpi);
        let width = (item.rcItem.right - item.rcItem.left) * 96 / self.dpi as i32;
        let height = (item.rcItem.bottom - item.rcItem.top) * 96 / self.dpi as i32;
        let disabled = item.itemState & ODS_DISABLED != 0;
        let active = child.id == INSTALL
            || self.inventory.as_ref().is_some_and(|i| {
                child.id == CHANNEL && i.channel == "stable"
                    || child.id == PREVIEW && i.channel == "preview"
            });
        c.rect(
            R::new(0, 0, width, height),
            if matches!(child.id, REFRESH | OFFLINE | CHANNEL | PREVIEW) {
                BG
            } else {
                PANEL
            },
        );
        let pressed = item.itemState & ODS_SELECTED != 0;
        let hovered = item.itemState & ODS_HOTLIGHT != 0;
        let fill = if disabled {
            FIELD
        } else if pressed {
            PRESSED
        } else if active {
            crate::paint::RED
        } else if hovered {
            HOVER
        } else {
            FIELD
        };
        c.round(
            R::new(0, 0, width - 1, height - 1),
            RADIUS_MD,
            fill,
            Some(if item.itemState & ODS_FOCUS != 0 {
                MUTED
            } else {
                BORDER
            }),
        );
        c.text(
            &child.label,
            R::new(0, 0, width, height),
            14,
            if active { 600 } else { 400 },
            if disabled { DIM } else { TEXT },
            DT_CENTER,
        );
    }
    unsafe fn row(&self, item: &DRAWITEMSTRUCT) {
        let physical_width = item.rcItem.right - item.rcItem.left;
        let physical_height = item.rcItem.bottom - item.rcItem.top;
        if physical_width <= 0 || physical_height <= 0 {
            return;
        }
        let dc = CreateCompatibleDC(item.hDC);
        let bitmap = CreateCompatibleBitmap(item.hDC, physical_width, physical_height);
        let previous = SelectObject(dc, bitmap);
        let brush = CreateSolidBrush(colorref(PANEL));
        let bounds = RECT {
            left: 0,
            top: 0,
            right: physical_width,
            bottom: physical_height,
        };
        FillRect(dc, &bounds, brush);
        DeleteObject(brush);
        {
            let c = Canvas::new(dc, self.dpi);
            let width = (item.rcItem.right - item.rcItem.left) * 96 / self.dpi as i32;
            let height = (item.rcItem.bottom - item.rcItem.top) * 96 / self.dpi as i32;
            if let Some(row) = self.inventory.as_ref().and_then(|i| {
                self.browser
                    .visible
                    .get(item.itemID as usize)
                    .and_then(|&index| i.row(index))
            }) {
                let checked = self.checked.contains(row.id);
                let focus = self.browser.visible.get(item.itemID as usize) == Some(&self.selected);
                let disabled = item.itemState & ODS_DISABLED != 0;
                c.round(
                    R::new(0, 2, width - 2, height - 4),
                    RADIUS_MD,
                    if checked {
                        blend(FIELD, DANGER, 0.12)
                    } else if focus {
                        FIELD
                    } else {
                        PANEL
                    },
                    Some(if checked {
                        DANGER
                    } else if focus && item.itemState & ODS_FOCUS != 0 {
                        WARN
                    } else {
                        BORDER
                    }),
                );
                c.round(
                    R::new(8, 11, 18, 18),
                    5,
                    if checked { DANGER } else { PANEL },
                    Some(if row.catalog_available { MUTED } else { BORDER }),
                );
                if checked {
                    c.text("✓", R::new(8, 10, 18, 20), 13, 600, TEXT, DT_CENTER);
                }
                let recommendation = self
                    .inventory
                    .as_ref()
                    .and_then(|i| i.recommendation.get(row.id).copied());
                c.ellipse(
                    R::new(40, 17, 6, 6),
                    match recommendation {
                        Some(Recommendation::Clear) => SUCCESS,
                        Some(Recommendation::Ambiguous) => WARN,
                        None => DIM,
                    },
                );
                let color = if disabled { DIM } else { TEXT };
                c.text(
                    row.name,
                    R::new(56, 4, width - 220, height - 8),
                    13,
                    500,
                    color,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                let installed = self
                    .inventory
                    .as_ref()
                    .and_then(|i| i.installed.get(row.id));
                c.text(
                    installed.map_or("未安装", Installation::label),
                    R::new(width - 155, 4, 140, height - 8),
                    12,
                    400,
                    match installed {
                        Some(Installation::Broken(_)) => WARN,
                        Some(Installation::Present(_)) => SUCCESS,
                        _ => DIM,
                    },
                    DT_RIGHT | DT_END_ELLIPSIS,
                );
            } else {
                c.text(
                    if self.browser.brand.is_some() || !self.browser.query.is_empty() {
                        "无匹配型号，试试其他厂商或搜索词。"
                    } else {
                        "暂无可用驱动，检查更新或切换通道。"
                    },
                    R::new(12, 4, width - 24, height - 8),
                    13,
                    400,
                    MUTED,
                    DT_LEFT,
                );
            }
        }
        BitBlt(
            item.hDC,
            item.rcItem.left,
            item.rcItem.top,
            physical_width,
            physical_height,
            dc,
            0,
            0,
            SRCCOPY,
        );
        SelectObject(dc, previous);
        DeleteObject(bitmap);
        DeleteDC(dc);
    }
}
unsafe extern "system" fn list_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(list_proc), 1);
    }
    let result = DefSubclassProc(hwnd, msg, w, l);
    if msg == WM_KEYUP {
        // Multiple-selection list boxes may move the caret without changing
        // any checks; keep single-item actions tied to the visible focus row.
        PostMessageW(GetParent(hwnd), FOCUS_CHANGED, 0, 0);
    }
    result
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Manager;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let manager = &mut *pointer;
    match message {
        WM_CREATE => {
            manager.layout(hwnd);
            manager.begin(
                hwnd,
                if manager.select_updates_after_refresh {
                    Operation::Refresh
                } else {
                    Operation::Load
                },
            );
            0
        }
        WM_PAINT => {
            manager.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if item.CtlType == ODT_LISTBOX && item.CtlID == LIST as u32 {
                manager.row(item);
            } else if item.CtlType == ODT_COMBOBOX {
                crate::paint::draw_native_choice(item, manager.dpi, manager.font);
            } else {
                manager.button(item);
            }
            1
        }
        WM_MEASUREITEM => {
            let item = &mut *(l as *mut MEASUREITEMSTRUCT);
            if item.CtlType == ODT_LISTBOX && item.CtlID == LIST as u32 {
                item.itemHeight = manager.px(40) as u32;
                1
            } else if crate::paint::measure_native_choice(item, manager.dpi) {
                1
            } else {
                0
            }
        }
        WM_COMMAND => {
            manager.command(hwnd, (w & 0xffff) as i32, ((w >> 16) & 0xffff) as u32);
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        FINISHED => {
            manager.finish(hwnd);
            0
        }
        FOCUS_CHANGED => {
            if manager.pending.is_none() && !manager.dialog_open {
                manager.sync_caret();
                manager.selection(hwnd);
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            SetTextColor(w as HDC, colorref(TEXT));
            if message == WM_CTLCOLORLISTBOX
                || (message == WM_CTLCOLORSTATIC
                    && matches!(GetDlgCtrlID(l as HWND), BRAND_LABEL | SEARCH_LABEL))
            {
                SetBkColor(w as HDC, colorref(PANEL));
                SetDCBrushColor(w as HDC, colorref(PANEL));
                GetStockObject(DC_BRUSH) as isize
            } else {
                SetBkColor(w as HDC, colorref(FIELD));
                manager.brush as isize
            }
        }
        WM_CLOSE => {
            if manager.pending.is_some() || manager.dialog_open {
                manager.status(
                    hwnd,
                    "后台操作尚未结束，请等待下载、校验或卸载完成后关闭。",
                    false,
                );
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}
/// The caller keeps the main application's modal flag set until this returns.
pub(crate) unsafe fn open(owner: HWND) -> StoreExit {
    open_inner(owner, false)
}

/// Opens the manager, refreshes the catalog, and selects installed packages
/// whose catalog version is strictly newer than the installed version.
pub(crate) unsafe fn open_updates(owner: HWND) -> StoreExit {
    open_inner(owner, true)
}

unsafe fn open_inner(owner: HWND, check_updates: bool) -> StoreExit {
    let monitor_dpi = GetDpiForWindow(owner).max(96);
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let ex_style = WS_EX_DLGMODALFRAME;
    let mut frame: RECT = zeroed();
    AdjustWindowRectExForDpi(&mut frame, style, 0, ex_style, monitor_dpi);
    let mut monitor: MONITORINFO = zeroed();
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    let work_area = (GetMonitorInfoW(
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) != 0)
        .then_some(monitor.rcWork);
    let dpi = work_area.map_or(monitor_dpi, |work| {
        let max_width = (work.right - work.left) - (frame.right - frame.left);
        let max_height = (work.bottom - work.top) - (frame.bottom - frame.top);
        monitor_dpi.min(
            (max_width * 96 / WIDTH)
                .min(max_height * 96 / HEIGHT)
                .max(1) as u32,
        )
    });
    let instance = GetModuleHandleW(null());
    let class_name = wide("PeripheralDeskDriverManager");
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class_name.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&class);
    let font = CreateFontW(
        -(14 * dpi as i32 / 96),
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Microsoft YaHei UI").as_ptr(),
    );
    let mut manager = Box::new(Manager {
        dpi,
        font,
        brush: CreateSolidBrush(colorref(FIELD)),
        children: Vec::new(),
        inventory: None,
        selected: 0,
        browser: Browser::default(),
        checked: BTreeSet::new(),
        pending: None,
        status: if check_updates {
            "正在检查驱动更新…".into()
        } else {
            "正在准备驱动目录…".into()
        },
        success: String::new(),
        error: false,
        dialog_open: false,
        changing_channel: false,
        select_updates_after_refresh: check_updates,
        changed: false,
        exit: StoreExit::Close,
    });
    let width = manager.px(WIDTH) + frame.right - frame.left;
    let height = manager.px(HEIGHT) + frame.bottom - frame.top;
    let mut parent = zeroed();
    GetWindowRect(owner, &mut parent);
    let x = parent.left + (parent.right - parent.left - width) / 2;
    let y = parent.top + (parent.bottom - parent.top - height) / 2;
    let (x, y) = work_area.map_or((x, y), |work| {
        (
            x.clamp(work.left, (work.right - width).max(work.left)),
            y.clamp(work.top, (work.bottom - height).max(work.top)),
        )
    });
    EnableWindow(owner, 0);
    let hwnd = CreateWindowExW(
        ex_style,
        class_name.as_ptr(),
        wide("外设盒 · 驱动管理").as_ptr(),
        style,
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        instance,
        manager.as_mut() as *mut Manager as _,
    );
    if !hwnd.is_null() {
        let dark = 1i32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as _,
            size_of::<i32>() as u32,
        );
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let mut message = zeroed();
        while IsWindow(hwnd) != 0 {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                if result == 0 {
                    PostQuitMessage(message.wParam as i32);
                }
                break;
            }
            if message.message == WM_KEYDOWN
                && message.wParam == usize::from(b'F')
                && GetKeyState(VK_CONTROL as i32) < 0
                && IsWindowEnabled(manager.child(SEARCH)) != 0
            {
                SetFocus(manager.child(SEARCH));
                SendMessageW(manager.child(SEARCH), EM_SETSEL, 0, -1);
                continue;
            }
            if IsDialogMessageW(hwnd, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
    }
    EnableWindow(owner, 1);
    SetForegroundWindow(owner);
    manager.exit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str) -> InstallTarget {
        InstallTarget {
            id: id.into(),
            name: format!("设备{id}"),
            version: "0.4.2".into(),
            bytes: 1024,
        }
    }

    #[test]
    fn batch_continues_after_failure_skips_same_and_reports_every_item() {
        let mut attempted = Vec::new();
        let mut progress = Vec::new();
        let result = install_batch(
            vec![target("a"), target("b"), target("c")],
            |id| (id == "b").then(|| "0.4.2".into()),
            |id| {
                attempted.push(id.to_string());
                if id == "a" {
                    Err("模拟哈希校验失败".into())
                } else {
                    Ok(())
                }
            },
            |index, count, item| progress.push((index, count, item.id.clone())),
        );
        assert_eq!(attempted, ["a", "c"]);
        assert_eq!(
            progress,
            [(1, 3, "a".into()), (2, 3, "b".into()), (3, 3, "c".into())]
        );
        assert!(result.failed());
        let description = result.description();
        assert!(description.contains("成功 1 项，失败 1 项，跳过已同版 1 项"));
        assert!(description.contains("失败 · 设备a：模拟哈希校验失败"));
        assert!(description.contains("跳过 · 设备b"));
        assert!(description.contains("成功 · 设备c"));
    }

    #[test]
    fn update_selection_includes_only_installed_strictly_older_versions() {
        let packages = [
            ("newer", "0.5.0"),
            ("same", "0.4.2"),
            ("older", "0.4.1"),
            ("missing", "0.9.0"),
            ("invalid", "not-a-version"),
        ]
        .map(|(id, version)| {
            serde_json::from_value::<DriverPackage>(serde_json::json!({
                "id":id, "name":format!("设备{id}"), "version":version, "description":"fixture", "download_bytes":1024,
                "package_url":"https://example.invalid/fixture.zip", "sha256":"fixture", "dll_sha256":"fixture", "device":{}
            })).unwrap()
        })
        .to_vec();
        let inventory = Inventory {
            packages,
            installed_only: vec![],
            installed: BTreeMap::from([
                ("newer".into(), Installation::Present("0.4.2".into())),
                ("same".into(), Installation::Present("0.4.2".into())),
                ("older".into(), Installation::Present("0.5.0".into())),
                ("invalid".into(), Installation::Present("0.4.2".into())),
            ]),
            recommendation: BTreeMap::new(),
            channel: "stable".into(),
            focus: None,
        };
        assert_eq!(updatable_ids(&inventory), BTreeSet::from(["newer".into()]));
    }

    #[test]
    fn clear_and_ambiguous_recommendations_are_separated() {
        let device = |id: &str, vid: u16, pid: u16| {
            serde_json::json!({
                "key": id, "name": id, "brand": "TEST", "kind": "mouse",
                "matches": [{"vid": vid, "pid": pid}]
            })
        };
        let mut packages = [
            ("clear-a", 0x1111u16, 0x2222u16),
            ("share-a", 0x3333, 0x4444),
            ("share-b", 0x3333, 0x4444),
            ("runtime-only", 0x5555, 0x6666),
        ]
        .map(|(id, vid, pid)| {
            serde_json::from_value::<DriverPackage>(serde_json::json!({
                "id": id, "name": id, "version": "0.4.2", "description": "fixture",
                "download_bytes": 0, "package_url": "https://example.invalid/f.zip",
                "sha256": "f", "dll_sha256": "f", "device": device(id, vid, pid)
            }))
            .unwrap()
        })
        .to_vec();
        packages[2].device["manual"] = true.into();
        packages[3].device["runtime_identity"] = true.into();
        let interfaces =
            [(0x1111u16, 0x2222u16), (0x3333, 0x4444), (0x5555, 0x6666)].map(|(vid, pid)| {
                crate::hid::Interface {
                    path: format!("\\\\?\\hid#{vid:04x}{pid:04x}"),
                    vid,
                    pid,
                    usage_page: 0,
                    usage: 0,
                    feature_len: 0,
                    input_len: 0,
                    output_len: 0,
                    product: String::new(),
                }
            });
        let recommendation = classify_detection(&packages, &interfaces);
        assert_eq!(recommendation.get("clear-a"), Some(&Recommendation::Clear));
        assert_eq!(
            recommendation.get("share-a"),
            Some(&Recommendation::Ambiguous)
        );
        assert_eq!(
            recommendation.get("share-b"),
            Some(&Recommendation::Ambiguous)
        );
        assert_eq!(
            recommendation.get("runtime-only"),
            Some(&Recommendation::Ambiguous)
        );
    }

    fn test_manager() -> Manager {
        let packages = ["a", "b"].map(|id| serde_json::from_value::<DriverPackage>(serde_json::json!({
            "id":id, "name":format!("设备{id}"), "version":"0.4.2", "description":"fixture", "download_bytes":1024,
            "package_url":"https://example.invalid/fixture.zip", "sha256":"fixture", "dll_sha256":"fixture", "device":{}
        })).unwrap()).to_vec();
        Manager {
            dpi: 96,
            font: null_mut(),
            brush: null_mut(),
            children: vec![],
            inventory: Some(Inventory {
                packages,
                installed_only: vec![],
                installed: BTreeMap::from([
                    ("a".into(), Installation::Broken("损坏记录".into())),
                    ("b".into(), Installation::Present("0.4.1".into())),
                ]),
                recommendation: BTreeMap::new(),
                channel: "stable".into(),
                focus: None,
            }),
            selected: 0,
            browser: Browser::default(),
            checked: BTreeSet::new(),
            pending: None,
            status: String::new(),
            success: String::new(),
            error: false,
            dialog_open: false,
            changing_channel: false,
            select_updates_after_refresh: false,
            changed: false,
            exit: StoreExit::Close,
        }
    }

    #[test]
    fn no_implicit_selection_and_broken_record_does_not_hide_other_targets() {
        let mut manager = test_manager();
        assert!(
            manager.targets().is_empty(),
            "detected devices must never be auto-selected"
        );
        assert!(matches!(manager.installation(), Installation::Broken(_)));
        manager
            .checked
            .extend(["a".into(), "b".into(), "not-in-catalog".into()]);
        assert_eq!(
            manager
                .targets()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        manager.selected = 1;
        assert!(matches!(manager.installation(), Installation::Present(_)));
    }

    #[test]
    fn catalog_browse_search_and_details_remain_optional_for_local_plugins() {
        let mut manager = test_manager();
        let inventory = manager.inventory.as_mut().unwrap();
        let mut entry = serde_json::to_value(&inventory.packages[0]).unwrap();
        entry["device"] = serde_json::json!({"brand": "Logitech", "name": "MX Anywhere 3"});
        entry["browse"] =
            serde_json::json!({"vendor_aliases": ["罗技", "羅技"], "family": "HID++ 2.0"});
        inventory.packages[0] = serde_json::from_value(entry).unwrap();
        inventory.packages[1].device = serde_json::json!({"brand": "Legacy", "name": "Mouse 9"});
        inventory.installed_only.push(
            serde_json::from_value(serde_json::json!({
                "id": "offline", "version": "1.0.0", "directory": "1.0.0",
                "manifest": {
                    "schema": 1, "abi": 1, "id": "offline", "version": "1.0.0", "name": "本地驱动",
                    "description": "旧版安装包", "dll": "driver.dll", "dll_sha256": "fixture",
                    "device": {"brand": "本地厂商", "name": "Keyboard 77"}
                }
            }))
            .unwrap(),
        );
        let mut browser = Browser::default();
        for (query, expected) in [
            ("罗技 hid++2.0", &[0][..]),
            ("羅技 ANYWHERE", &[0]),
            ("Legacy mouse9", &[1]),
            ("本地厂商 keyboard77", &[2]),
            ("罗技 mouse9", &[]),
        ] {
            browser.query = query.into();
            browser.filter(Some(inventory));
            assert_eq!(browser.visible, expected, "{query}");
        }
        let details = inventory.row(0).unwrap().details();
        assert!(details.contains("厂商别名：罗技、羅技"));
        assert!(details.contains("协议系列：HID++ 2.0"));
        for row in [1, 2] {
            let details = inventory.row(row).unwrap().details();
            assert!(!details.contains("厂商别名"));
            assert!(!details.contains("协议系列"));
        }
    }

    #[test]
    fn native_filters_and_batch_actions_keep_the_model_identity() {
        let mut manager = test_manager();
        let inventory = manager.inventory.as_mut().unwrap();
        inventory.packages[0].device =
            serde_json::json!({"brand":"MCHOSE", "name":"迈从 A7 V3 Ultra"});
        inventory.packages[1].device =
            serde_json::json!({"brand":"Mchose", "name":"迈从 A7 V3 Ultra+"});
        let mut third = inventory.packages[0].clone();
        third.id = "c".into();
        third.device = serde_json::json!({"brand":"Razer", "name":"Viper V3 Pro"});
        inventory.packages.push(third);
        inventory
            .recommendation
            .insert("a".into(), Recommendation::Clear);
        inventory
            .recommendation
            .insert("b".into(), Recommendation::Ambiguous);
        unsafe {
            let window = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                WIDTH,
                HEIGHT,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!window.is_null());
            manager.layout(window);
            assert_eq!(manager.browser.brands, ["MCHOSE", "Razer"]);
            SendMessageW(manager.child(BRAND), CB_SETCURSEL, 1, 0);
            manager.command(window, BRAND, CBN_SELCHANGE);
            assert_ne!(
                GetWindowLongW(manager.child(MHUB), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            manager.command(window, SELECT_DETECTED, 0);
            assert_eq!(manager.checked, BTreeSet::from(["a".into()]));

            SetWindowTextW(manager.child(SEARCH), wide("ultra+ A7v3").as_ptr());
            manager.command(window, SEARCH, EN_CHANGE);
            assert_eq!(manager.browser.visible, [1]);
            assert_eq!(manager.selected().unwrap().id, "b");
            SendMessageW(manager.child(LIST), LB_SETSEL, 1, 0);
            manager.command(window, LIST, LBN_SELCHANGE);
            assert_eq!(manager.checked, BTreeSet::from(["a".into(), "b".into()]));

            SendMessageW(manager.child(BRAND), CB_SETCURSEL, 2, 0);
            manager.command(window, BRAND, CBN_SELCHANGE);
            assert!(manager.browser.visible.is_empty());
            assert!(manager.selected().is_none());
            assert_eq!(
                GetWindowLongW(manager.child(MHUB), GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            SetWindowTextW(manager.child(SEARCH), wide("viPEr V3").as_ptr());
            manager.command(window, SEARCH, EN_CHANGE);
            assert_eq!(manager.selected().unwrap().id, "c");
            manager.command(window, SELECT_ALL, 0);
            assert_eq!(
                manager
                    .targets()
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b", "c"]
            );
            manager.command(window, SELECT_NONE, 0);
            assert!(manager.targets().is_empty());
            SendMessageW(manager.child(BRAND), CB_SETCURSEL, 1, 0);
            manager.command(window, BRAND, CBN_SELCHANGE);
            manager.command(window, MHUB, BN_CLICKED);
            assert!(manager.exit == StoreExit::MHub);
            assert_eq!(IsWindow(window), 0);
        }
    }
}
