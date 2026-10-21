//! On-demand native driver-package manager. Package verification stays in the library.
use crate::{
    hid::wide,
    paint::{Canvas, BG, BORDER, DIM, FIELD, GREEN, MUTED, ORANGE, PANEL, R, RED, TEXT},
};
use peripheral_desk::plugin_manager::{self, DriverPackage, InstalledDriver};
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
            "安装完成：成功 {installed} 项，失败 {failed} 项，跳过已同版 {skipped} 项。新设备安装后请重启主程序。"
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
    inventory: Result<Inventory, String>,
    batch: Option<BatchResult>,
}
enum WorkEvent {
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
    checked: BTreeSet<String>,
    pending: Option<Receiver<WorkEvent>>,
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
        if matches!(class, "EDIT" | "LISTBOX") {
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
        for (id, label, x) in [(CHANNEL, "稳定版", 518), (PREVIEW, "预览版", 602)] {
            self.add(
                hwnd,
                id,
                "BUTTON",
                label,
                R::new(x, 26, 76, 38),
                BS_OWNERDRAW as u32,
            );
        }
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
            R::new(40, 176, 960, 164),
            LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32
                | LBS_OWNERDRAWFIXED as u32
                | LBS_HASSTRINGS as u32
                | LBS_MULTIPLESEL as u32
                | WS_VSCROLL,
        );
        SendMessageW(self.child(LIST), LB_SETITEMHEIGHT, 0, self.px(40) as isize);
        if let Some(inventory) = &self.inventory {
            for index in 0..inventory.len() {
                let package = inventory.row(index).unwrap();
                SendMessageW(
                    self.child(LIST),
                    LB_ADDSTRING,
                    0,
                    wide(package.name).as_ptr() as isize,
                );
                SendMessageW(
                    self.child(LIST),
                    LB_SETSEL,
                    usize::from(self.checked.contains(package.id)),
                    index as isize,
                );
            }
            if inventory.len() > 0 {
                self.selected = self.selected.min(inventory.len() - 1);
                SendMessageW(self.child(LIST), LB_SETCARETINDEX, self.selected, 0);
            }
        }
        if self.inventory.as_ref().is_none_or(|i| i.len() == 0) {
            SendMessageW(
                self.child(LIST),
                LB_ADDSTRING,
                0,
                wide("暂无可用驱动").as_ptr() as isize,
            );
        }
        for (id, label, x, width) in [
            (SELECT_ALL, "全选", 44, 84),
            (SELECT_NONE, "取消选择", 138, 108),
            (SELECT_DETECTED, "选择已识别设备", 256, 170),
        ] {
            self.add(
                hwnd,
                id,
                "BUTTON",
                label,
                R::new(x, 110, width, 32),
                BS_OWNERDRAW as u32,
            );
        }

        self.add(
            hwnd,
            DESCRIPTION,
            "EDIT",
            "",
            R::new(44, 454, 952, 56),
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
        EnableWindow(self.child(REFRESH), ready as i32);
        EnableWindow(self.child(LIST), ready as i32);
        EnableWindow(self.child(OFFLINE), ready as i32);
        EnableWindow(self.child(CHANNEL), ready as i32);
        EnableWindow(self.child(PREVIEW), ready as i32);
        for id in [SELECT_ALL, SELECT_NONE, SELECT_DETECTED] {
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
            Operation::Refresh => ("正在连接驱动仓库，刷新可用版本…", "驱动仓库已刷新。"),
            Operation::Install(_) => (
                "正在逐项下载并校验已选驱动，请等待完成…",
                "批量安装结束，请查看各项结果。新增设备支持请重启主程序。",
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
            let mut batch = None;
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
                    inventory(false)
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
            if tx
                .send(WorkEvent::Finished(WorkResult {
                    inventory: result,
                    batch,
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
            Ok(WorkEvent::Progress(progress)) => {
                self.status(hwnd, progress, false);
                return;
            }
            Ok(WorkEvent::Finished(result)) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(error) => WorkResult {
                inventory: Err(format!("后台任务未返回结果：{error}")),
                batch: None,
            },
        };
        self.pending = None;
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
        if matches!(id, REMOVE | ROLLBACK | CHANGELOG) {
            let caret = SendMessageW(self.child(LIST), LB_GETCARETINDEX, 0, 0);
            if caret >= 0 {
                self.selected = caret as usize;
            }
        }
        match id {
            LIST if event == LBN_SELCHANGE => {
                self.selected = SendMessageW(self.child(LIST), LB_GETCARETINDEX, 0, 0) as usize;
                if let Some(inventory) = &self.inventory {
                    self.checked.clear();
                    for index in 0..inventory.len() {
                        let row = inventory.row(index).unwrap();
                        if row.catalog_available
                            && SendMessageW(self.child(LIST), LB_GETSEL, index, 0) > 0
                        {
                            self.checked.insert(row.id.to_string());
                        } else if !row.catalog_available {
                            SendMessageW(self.child(LIST), LB_SETSEL, 0, index as isize);
                        }
                    }
                }
                self.selection(hwnd);
            }
            SELECT_ALL | SELECT_NONE | SELECT_DETECTED => {
                self.checked.clear();
                if let Some(inventory) = &self.inventory {
                    for index in 0..inventory.len() {
                        let row = inventory.row(index).unwrap();
                        let checked = row.catalog_available
                            && (id == SELECT_ALL
                                || id == SELECT_DETECTED
                                    && inventory.detected.get(row.id) == Some(&true));
                        if checked {
                            self.checked.insert(row.id.to_string());
                        }
                        SendMessageW(
                            self.child(LIST),
                            LB_SETSEL,
                            usize::from(checked),
                            index as isize,
                        );
                    }
                }
                self.selection(hwnd);
            }
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
            c.gradient(R::new(0, 0, WIDTH, 84), 0xff201017, BG);
            c.text("驱动管理", R::new(28, 18, 390, 32), 23, 600, TEXT, DT_LEFT);
            c.text("更新通道", R::new(426, 30, 84, 24), 13, 400, MUTED, DT_LEFT);
            c.text(
                "按型号勾选，一次安装多个驱动。点击行即可勾选，无需 Ctrl。",
                R::new(28, 55, 500, 24),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.round(R::new(24, 98, 992, 258), 18, PANEL, Some(BORDER));
            let targets = self.targets();
            let bytes = targets.iter().map(|t| t.bytes).sum();
            c.text(
                &format!(
                    "已选 {} 项 · 包体积 {} · 已同版将跳过",
                    targets.len(),
                    if targets.is_empty() {
                        "—".into()
                    } else {
                        size(bytes)
                    }
                ),
                R::new(452, 114, 540, 24),
                12,
                400,
                MUTED,
                DT_RIGHT,
            );
            for (x, width, label) in [
                (82, 310, "设备接口 · 实心圆表示已识别"),
                (410, 150, "可用版本"),
                (580, 180, "已安装版本"),
                (810, 176, "包体积"),
            ] {
                c.text(
                    label,
                    R::new(x, 148, width, 22),
                    12,
                    500,
                    DIM,
                    if label == "包体积" {
                        DT_RIGHT
                    } else {
                        DT_LEFT
                    },
                );
            }
            if self.inventory.as_ref().is_none_or(|i| i.len() == 0) {
                c.text(
                    "当前通道暂无可用驱动，请刷新仓库或切换通道。",
                    R::new(52, 230, 936, 28),
                    14,
                    400,
                    MUTED,
                    DT_CENTER,
                );
            }
            c.round(R::new(24, 372, 992, 216), 18, PANEL, Some(BORDER));
            c.round(R::new(36, 448, 968, 68), 10, FIELD, Some(BORDER));
            if let Some(package) = self.selected() {
                c.text(
                    package.name,
                    R::new(44, 384, 760, 30),
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
                    R::new(44, 420, 952, 24),
                    13,
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
                    "选择一行查看详情，勾选多个型号安装",
                    R::new(44, 384, 952, 32),
                    21,
                    600,
                    TEXT,
                    DT_LEFT,
                );
                c.text(
                    "只展示仓库提供的信息；尚未下载的驱动显示为未安装。",
                    R::new(44, 420, 952, 24),
                    14,
                    400,
                    MUTED,
                    DT_LEFT,
                );
            }
            c.text(
                "新增设备安装后重启主程序。卸载与回退只操作当前焦点行，保留配置。",
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
                if self.error { ORANGE } else { DIM },
                DT_LEFT,
            );
            c.round(R::new(24, 634, 992, 52), 10, FIELD, Some(BORDER));
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
            if let Some(row) = self
                .inventory
                .as_ref()
                .and_then(|i| i.row(item.itemID as usize))
            {
                let checked = self.checked.contains(row.id);
                let focus = item.itemID as usize == self.selected;
                let disabled = item.itemState & ODS_DISABLED != 0;
                c.round(
                    R::new(0, 2, width - 2, height - 4),
                    9,
                    if checked {
                        0xff2b1822
                    } else if focus {
                        FIELD
                    } else {
                        PANEL
                    },
                    Some(if checked {
                        RED
                    } else if focus && item.itemState & ODS_FOCUS != 0 {
                        ORANGE
                    } else {
                        BORDER
                    }),
                );
                c.round(
                    R::new(8, 11, 18, 18),
                    5,
                    if checked { RED } else { PANEL },
                    Some(if row.catalog_available { MUTED } else { BORDER }),
                );
                if checked {
                    c.text("✓", R::new(8, 10, 18, 20), 13, 600, TEXT, DT_CENTER);
                }
                let detected = self
                    .inventory
                    .as_ref()
                    .is_some_and(|i| i.detected.get(row.id) == Some(&true));
                c.ellipse(R::new(40, 17, 6, 6), if detected { GREEN } else { DIM });
                let color = if disabled { DIM } else { TEXT };
                c.text(
                    row.name,
                    R::new(56, 4, 300, height - 8),
                    13,
                    500,
                    color,
                    DT_LEFT,
                );
                c.text(
                    if row.catalog_available {
                        row.version
                    } else {
                        "无可用更新"
                    },
                    R::new(370, 4, 154, height - 8),
                    12,
                    400,
                    MUTED,
                    DT_LEFT,
                );
                let installed = self
                    .inventory
                    .as_ref()
                    .and_then(|i| i.installed.get(row.id));
                c.text(
                    installed.map_or("未安装", Installation::label),
                    R::new(540, 4, 210, height - 8),
                    12,
                    400,
                    if matches!(installed, Some(Installation::Broken(_))) {
                        ORANGE
                    } else if matches!(installed, Some(Installation::Present(_))) {
                        GREEN
                    } else {
                        DIM
                    },
                    DT_LEFT,
                );
                c.text(
                    &if row.catalog_available {
                        size(row.download_bytes)
                    } else {
                        "—".into()
                    },
                    R::new(770, 4, width - 784, height - 8),
                    12,
                    400,
                    MUTED,
                    DT_RIGHT,
                );
            } else {
                c.text(
                    "当前通道暂无可用驱动，请刷新仓库或切换通道。",
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
            manager.begin(hwnd, Operation::Load);
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
                let caret = SendMessageW(manager.child(LIST), LB_GETCARETINDEX, 0, 0);
                if caret >= 0 {
                    manager.selected = caret as usize;
                    manager.selection(hwnd);
                }
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            SetTextColor(w as HDC, colorref(TEXT));
            if message == WM_CTLCOLORLISTBOX {
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
        checked: BTreeSet::new(),
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
    fn no_implicit_selection_and_broken_record_does_not_hide_other_targets() {
        let packages = ["a", "b"].map(|id| serde_json::from_value::<DriverPackage>(serde_json::json!({
            "id":id, "name":format!("设备{id}"), "version":"0.4.2", "description":"fixture", "download_bytes":1024,
            "package_url":"https://example.invalid/fixture.zip", "sha256":"fixture", "dll_sha256":"fixture", "device":{}
        })).unwrap()).to_vec();
        let mut manager = Manager {
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
                detected: BTreeMap::from([("b".into(), true)]),
                channel: "stable".into(),
                focus: None,
            }),
            selected: 0,
            checked: BTreeSet::new(),
            pending: None,
            status: String::new(),
            success: String::new(),
            error: false,
            dialog_open: false,
            changing_channel: false,
        };
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
}
