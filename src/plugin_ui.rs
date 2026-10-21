//! On-demand native driver-package manager. Package verification stays in the library.
use crate::{
    hid::wide,
    paint::{Canvas, BG, BORDER, DIM, FIELD, GREEN, MUTED, ORANGE, PANEL, R, RED, TEXT},
};
use peripheral_desk::plugin_manager::{self, DriverPackage, InstalledDriver};
use std::{
    collections::BTreeMap,
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
        WindowsAndMessaging::*,
    },
};
const WIDTH: i32 = 1040;
const HEIGHT: i32 = 700;
const FINISHED: u32 = WM_APP + 84;
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
struct Inventory {
    packages: Vec<DriverPackage>,
    installed_only: Vec<InstalledDriver>,
    installed: BTreeMap<String, Installation>,
    detected: BTreeMap<String, bool>,
    channel: String,
    focus: Option<String>,
}
struct DriverRow<'a> {
    id: &'a str,
    name: &'a str,
    version: &'a str,
    description: &'a str,
    changelog: &'a str,
    download_bytes: u64,
    catalog_available: bool,
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
                download_bytes: package.download_bytes,
                catalog_available: true,
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
            download_bytes: 0,
            catalog_available: false,
        })
    }
}
enum Operation {
    Load,
    Refresh,
    Install(String),
    Remove(String),
    Offline(PathBuf),
    Channel(String),
    Rollback(String),
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
    pending: Option<Receiver<Result<Inventory, String>>>,
    status: String,
    success: String,
    error: bool,
    dialog_open: bool,
    changing_channel: bool,
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
fn inventory(refresh: bool) -> Result<Inventory, String> {
    let packages = if refresh {
        plugin_manager::refresh_catalog()
    } else {
        plugin_manager::catalog()
    }
    .map_err(|e| e.to_string())?;
    let interfaces = crate::hid::enumerate()?;
    let detected = packages
        .iter()
        .map(|p| {
            let found = serde_json::from_value::<crate::devices::DeviceManifest>(p.device.clone())
                .ok()
                .is_some_and(|d| {
                    interfaces
                        .iter()
                        .any(|i| d.matches.iter().any(|m| m.matches(i)))
                });
            (p.id.clone(), found)
        })
        .collect();
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
    let installed_only = plugin_manager::installed_all()?
        .into_iter()
        .filter(|driver| !packages.iter().any(|package| package.id == driver.id))
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
        detected,
        installed,
        channel: plugin_manager::channel(),
        focus: None,
    })
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
    fn installation(&self) -> &Installation {
        static MISSING: Installation = Installation::Missing;
        self.selected()
            .and_then(|p| self.inventory.as_ref()?.installed.get(p.id))
            .unwrap_or(&MISSING)
    }
    unsafe fn add(&mut self, hwnd: HWND, id: i32, class: &str, label: &str, r: R, style: u32) {
        let child = CreateWindowExW(
            if class == "EDIT" || class == "LISTBOX" || class == "COMBOBOX" {
                WS_EX_CLIENTEDGE
            } else {
                0
            },
            wide(class).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD | WS_TABSTOP | style,
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
        self.add(
            hwnd,
            CHANNEL,
            "COMBOBOX",
            "",
            R::new(548, 28, 126, 210),
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        );
        for label in ["稳定版", "预览版"] {
            SendMessageW(
                self.child(CHANNEL),
                CB_ADDSTRING,
                0,
                wide(label).as_ptr() as isize,
            );
        }
        let selected = self
            .inventory
            .as_ref()
            .map(|s| if s.channel == "preview" { 1 } else { 0 })
            .unwrap_or(usize::MAX);
        SendMessageW(self.child(CHANNEL), CB_SETCURSEL, selected, 0);
        self.add(
            hwnd,
            OFFLINE,
            "BUTTON",
            "离线安装…",
            R::new(688, 26, 136, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            REFRESH,
            "BUTTON",
            "刷新驱动仓库",
            R::new(836, 26, 176, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            LIST,
            "LISTBOX",
            "",
            R::new(40, 160, 960, 140),
            LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32
                | LBS_USETABSTOPS as u32
                | WS_VSCROLL
                | WS_HSCROLL,
        );
        let stops = [168i32, 264, 391];
        SendMessageW(
            self.child(LIST),
            LB_SETTABSTOPS,
            stops.len(),
            stops.as_ptr() as isize,
        );
        SendMessageW(self.child(LIST), LB_SETITEMHEIGHT, 0, self.px(28) as isize);
        SendMessageW(
            self.child(LIST),
            LB_SETHORIZONTALEXTENT,
            self.px(1100) as usize,
            0,
        );
        if let Some(inventory) = &self.inventory {
            for index in 0..inventory.len() {
                let package = inventory.row(index).unwrap();
                let state = inventory
                    .installed
                    .get(package.id)
                    .map_or("未安装", Installation::label);
                let name = if package.catalog_available {
                    format!(
                        "{} {}",
                        if inventory.detected.get(package.id) == Some(&true) {
                            "●"
                        } else {
                            "○"
                        },
                        package.name
                    )
                } else {
                    format!("{}（已安装）", package.name)
                };
                let row = format!(
                    "{}\t{}\t{}\t{}",
                    name,
                    package.version,
                    state,
                    if package.catalog_available {
                        size(package.download_bytes)
                    } else {
                        "—".into()
                    }
                );
                SendMessageW(
                    self.child(LIST),
                    LB_ADDSTRING,
                    0,
                    wide(&row).as_ptr() as isize,
                );
            }
            if inventory.len() > 0 {
                self.selected = self.selected.min(inventory.len() - 1);
                SendMessageW(self.child(LIST), LB_SETCURSEL, self.selected, 0);
            }
        }

        self.add(
            hwnd,
            DESCRIPTION,
            "EDIT",
            "",
            R::new(44, 424, 952, 76),
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
            "卸载（保留配置）",
            R::new(268, 532, 208, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            ROLLBACK,
            "BUTTON",
            "回退上一版",
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
    unsafe fn selection(&mut self, hwnd: HWND) {
        let description = self
            .selected()
            .map(|p| format!("{}\r\n包标识：{}", p.description, p.id))
            .unwrap_or_else(|| "目录尚不可用，请查看操作结果，或点击“刷新驱动仓库”。".into());
        let detail = match self.installation() {
            Installation::Broken(error) => format!("{description}\r\n安装记录异常：{error}"),
            _ => description,
        };
        self.text(DESCRIPTION, &detail);
        let label = match (self.selected(), self.installation()) {
            (Some(package), _) if !package.catalog_available => "当前通道无可用更新",
            (Some(package), Installation::Present(version)) if *version == package.version => {
                "已安装此版本"
            }
            (Some(_), Installation::Present(_)) => "更新所选驱动",
            _ => "安装所选驱动",
        };
        if let Some(child) = self.children.iter_mut().find(|c| c.id == INSTALL) {
            child.label = label.into();
        }
        self.text(INSTALL, label);
        self.availability();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn availability(&self) {
        let ready = self.pending.is_none() && !self.dialog_open;
        EnableWindow(self.child(REFRESH), ready as i32);
        EnableWindow(self.child(LIST), ready as i32);
        EnableWindow(self.child(OFFLINE), ready as i32);
        EnableWindow(self.child(CHANNEL), ready as i32);
        EnableWindow(
            self.child(CHANGELOG),
            (ready && self.selected().is_some()) as i32,
        );
        EnableWindow(
            self.child(ROLLBACK),
            (ready && matches!(self.installation(), Installation::Present(_))) as i32,
        );
        let same = matches!((self.selected(),self.installation()),(Some(package),Installation::Present(version))if *version==package.version);
        EnableWindow(
            self.child(INSTALL),
            (ready && self.selected().is_some_and(|p| p.catalog_available) && !same) as i32,
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
        let (progress, success) = match &operation {
            Operation::Load => (
                "正在读取驱动目录…",
                "目录已加载。请按需安装所选驱动，不会自动安装其它设备。",
            ),
            Operation::Refresh => ("正在连接驱动仓库，刷新可用版本…", "驱动仓库已刷新。"),
            Operation::Install(_) => (
                "正在下载并校验所选驱动，请等待完成…",
                "所选驱动已安装。已有设备关闭窗口后刷新；新设备支持请重启主程序。",
            ),
            Operation::Remove(_) => (
                "正在卸载所选驱动，保留用户配置…",
                "所选驱动已卸载，配置已保留。关闭窗口后刷新设备；重启可完全释放已加载的驱动。",
            ),
            Operation::Offline(_) => (
                "正在校验离线驱动包并安装…",
                "离线驱动已安装。已有设备关闭窗口后刷新；新增设备支持请重启主程序。",
            ),
            Operation::Channel(_) => (
                "正在切换通道并刷新驱动仓库…",
                "更新通道已切换，驱动目录已刷新。",
            ),
            Operation::Rollback(_) => (
                "正在恢复所选驱动的上一版本…",
                "驱动已回退。关闭窗口后刷新设备；已加载的驱动可能需要重启主程序。",
            ),
        };
        self.success = success.into();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status(hwnd, progress, false);
        self.availability();
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = (|| match operation {
                Operation::Load => inventory(false),
                Operation::Refresh => inventory(true),
                Operation::Install(id) => {
                    let installed = plugin_manager::install(&id).map_err(|e| e.to_string())?;
                    let mut state = inventory(false)?;
                    state.focus = Some(installed.id);
                    Ok(state)
                }
                Operation::Offline(path) => {
                    let installed =
                        plugin_manager::install_offline(&path).map_err(|e| e.to_string())?;
                    let mut state = inventory(false)?;
                    state.focus = Some(installed.id);
                    Ok(state)
                }
                Operation::Rollback(id) => {
                    let installed = plugin_manager::rollback(&id).map_err(|e| e.to_string())?;
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
                    inventory(false)
                }
            })();
            if tx.send(result).is_ok() {
                unsafe {
                    PostMessageW(window as HWND, FINISHED, 0, 0);
                }
            }
        });
    }
    unsafe fn finish(&mut self, hwnd: HWND) {
        let Some(receiver) = self.pending.take() else {
            return;
        };
        let selected = self.selected().map(|p| p.id.to_string());
        match receiver.try_recv() {
            Ok(Ok(inventory)) => {
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
                self.status = self.success.clone();
                self.error = false;
                self.layout(hwnd);
            }
            Ok(Err(error)) => {
                self.status(hwnd, format!("操作未完成：{error}"), true);
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
            Err(error) => {
                self.status(hwnd, format!("后台任务未返回结果：{error}"), true);
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
    }
    unsafe fn offline(&mut self, hwnd: HWND) {
        if self.pending.is_some() || self.dialog_open {
            return;
        }
        let mut file = [0u16; 32768];
        let filter = wide("Peripheral Desk 驱动包 (*.zip)\0*.zip\0\0");
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
        match id {
            LIST if event == LBN_SELCHANGE => {
                self.selected = SendMessageW(self.child(LIST), LB_GETCURSEL, 0, 0) as usize;
                self.selection(hwnd);
            }
            REFRESH => self.begin(hwnd, Operation::Refresh),
            OFFLINE => self.offline(hwnd),
            CHANNEL if event == CBN_SELCHANGE => {
                let index = SendMessageW(self.child(CHANNEL), CB_GETCURSEL, 0, 0);
                if index == 0 || index == 1 {
                    self.begin(
                        hwnd,
                        Operation::Channel(if index == 1 { "preview" } else { "stable" }.into()),
                    );
                }
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
                if let Some(package) = self.selected() {
                    if package.catalog_available {
                        self.begin(hwnd, Operation::Install(package.id.to_string()));
                    }
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
            c.gradient(R::new(0, 0, WIDTH, 84), 0xff201017, BG);
            c.text("驱动管理", R::new(28, 18, 390, 32), 23, 600, TEXT, DT_LEFT);
            c.text("更新通道", R::new(448, 30, 92, 24), 13, 400, MUTED, DT_LEFT);
            c.text(
                "从独立驱动仓库按需安装，让主程序保持轻量。",
                R::new(28, 55, 500, 24),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
            c.round(R::new(24, 108, 992, 208), 14, PANEL, Some(BORDER));
            for (x, width, label) in [
                (42, 310, "设备接口 ● 已发现 / ○ 未连接"),
                (336, 150, "可用版本"),
                (506, 180, "已安装版本"),
                (724, 250, "下载大小"),
            ] {
                c.text(label, R::new(x, 124, width, 24), 13, 600, MUTED, DT_LEFT);
            }
            c.round(R::new(24, 334, 992, 254), 14, PANEL, Some(BORDER));
            if let Some(package) = self.selected() {
                c.text(
                    package.name,
                    R::new(44, 352, 760, 30),
                    21,
                    600,
                    TEXT,
                    DT_LEFT,
                );
                c.text(
                    &format!(
                        "{} · 可用 {} · 已安装 {} · {}",
                        if !package.catalog_available {
                            "仅本机已安装"
                        } else if self
                            .inventory
                            .as_ref()
                            .is_some_and(|i| i.detected.get(package.id) == Some(&true))
                        {
                            "已发现设备接口"
                        } else {
                            "未发现设备接口"
                        },
                        package.version,
                        self.installation().label(),
                        if package.catalog_available {
                            size(package.download_bytes)
                        } else {
                            "—".into()
                        }
                    ),
                    R::new(44, 392, 952, 25),
                    14,
                    400,
                    if matches!(self.installation(), Installation::Present(_)) {
                        GREEN
                    } else {
                        MUTED
                    },
                    DT_LEFT,
                );
            } else {
                c.text(
                    "选择要安装的设备驱动",
                    R::new(44, 352, 952, 32),
                    21,
                    600,
                    TEXT,
                    DT_LEFT,
                );
                c.text(
                    "只展示仓库提供的信息；尚未下载的驱动显示为未安装。",
                    R::new(44, 392, 952, 25),
                    14,
                    400,
                    MUTED,
                    DT_LEFT,
                );
            }
            c.text(
                "已有设备：关闭窗口后刷新设备。新增设备支持：安装后重启主程序。",
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
                    "操作结果"
                },
                R::new(28, 612, 984, 21),
                12,
                400,
                if self.error { ORANGE } else { DIM },
                DT_LEFT,
            );
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
        let background = if matches!(child.id, REFRESH | OFFLINE) {
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
        let active = child.id == INSTALL;
        c.rect(
            R::new(0, 0, width, height),
            if matches!(child.id, REFRESH | OFFLINE) {
                BG
            } else {
                PANEL
            },
        );
        c.round(
            R::new(0, 0, width - 1, height - 1),
            9,
            if disabled || !active {
                FIELD
            } else if item.itemState & ODS_SELECTED != 0 {
                ORANGE
            } else {
                RED
            },
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
            manager.begin(hwnd, Operation::Load);
            0
        }
        WM_PAINT => {
            manager.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DRAWITEM => {
            manager.button(&*(l as *const DRAWITEMSTRUCT));
            1
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
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            SetTextColor(w as HDC, colorref(TEXT));
            SetBkColor(w as HDC, colorref(FIELD));
            manager.brush as isize
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
pub unsafe fn open(owner: HWND) {
    let dpi = GetDpiForWindow(owner).max(96);
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
        pending: None,
        status: "正在准备驱动目录…".into(),
        success: String::new(),
        error: false,
        dialog_open: false,
        changing_channel: false,
    });
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: manager.px(WIDTH),
        bottom: manager.px(HEIGHT),
    };
    AdjustWindowRectExForDpi(&mut frame, style, 0, 0, dpi);
    let mut parent = zeroed();
    GetWindowRect(owner, &mut parent);
    let x = (parent.left + (parent.right - parent.left - (frame.right - frame.left)) / 2).max(0);
    let y = (parent.top + (parent.bottom - parent.top - (frame.bottom - frame.top)) / 2).max(0);
    EnableWindow(owner, 0);
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class_name.as_ptr(),
        wide("Peripheral Desk · 驱动管理").as_ptr(),
        style,
        x,
        y,
        frame.right - frame.left,
        frame.bottom - frame.top,
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
}
