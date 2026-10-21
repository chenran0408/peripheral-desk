//! Independent core release channel and restart-aware native update window.
use crate::{
    hid::wide,
    paint::{Canvas, BG, BORDER, DIM, FIELD, GREEN, MUTED, ORANGE, PANEL, R, RED, TEXT},
};
use peripheral_desk::core_update::{self, UpdateInfo};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::LibraryLoader::*,
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
const WIDTH: i32 = 940;
const HEIGHT: i32 = 700;
const DONE: u32 = WM_APP + 86;
const CHANNEL: i32 = 101;
const CHECK: i32 = 102;
const INSTALL: i32 = 103;
const ROLLBACK: i32 = 104;
const NOTES: i32 = 105;
const STATUS: i32 = 106;
enum Operation {
    Inspect,
    Check(String),
    Install(UpdateInfo),
    Rollback,
}
enum Outcome {
    Inspected(Option<String>),
    Checked(UpdateInfo),
    Restart,
}
struct Child {
    hwnd: HWND,
    id: i32,
    label: String,
}
struct Updater {
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    children: Vec<Child>,
    info: Option<UpdateInfo>,
    previous: Option<String>,
    pending: Option<Receiver<Result<Outcome, String>>>,
    status: String,
    error: bool,
    restart: bool,
}
impl Drop for Updater {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.brush);
        }
    }
}
fn rgb(argb: u32) -> u32 {
    ((argb >> 16) & 255) | (argb & 0xff00) | ((argb & 255) << 16)
}
fn size(bytes: u64) -> String {
    if bytes == 0 {
        "—".into()
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}
impl Updater {
    fn px(&self, n: i32) -> i32 {
        (n as i64 * self.dpi as i64 / 96) as i32
    }
    fn child(&self, id: i32) -> HWND {
        self.children
            .iter()
            .find(|c| c.id == id)
            .map_or(null_mut(), |c| c.hwnd)
    }
    unsafe fn add(&mut self, owner: HWND, id: i32, class: &str, text: &str, r: R, style: u32) {
        let child = CreateWindowExW(
            if class == "EDIT" || class == "COMBOBOX" {
                WS_EX_CLIENTEDGE
            } else {
                0
            },
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_TABSTOP | style,
            self.px(r.x),
            self.px(r.y),
            self.px(r.w),
            self.px(r.h),
            owner,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(child, WM_SETFONT, self.font as usize, 0);
        self.children.push(Child {
            hwnd: child,
            id,
            label: text.into(),
        });
    }
    unsafe fn layout(&mut self, hwnd: HWND) {
        self.add(
            hwnd,
            CHANNEL,
            "COMBOBOX",
            "",
            R::new(644, 28, 128, 200),
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
        SendMessageW(
            self.child(CHANNEL),
            CB_SETCURSEL,
            if core_update::channel() == "preview" {
                1
            } else {
                0
            },
            0,
        );
        self.add(
            hwnd,
            CHECK,
            "BUTTON",
            "检查更新",
            R::new(788, 26, 124, 38),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            NOTES,
            "EDIT",
            "选择通道并检查主程序更新。此处独立于设备驱动更新。",
            R::new(44, 300, 852, 210),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        self.add(
            hwnd,
            INSTALL,
            "BUTTON",
            "下载更新并重启",
            R::new(44, 550, 280, 42),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            ROLLBACK,
            "BUTTON",
            "回退上一版并重启",
            R::new(344, 550, 280, 42),
            BS_OWNERDRAW as u32,
        );
        self.add(
            hwnd,
            STATUS,
            "EDIT",
            &self.status.clone(),
            R::new(28, 640, 884, 42),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        for child in &self.children {
            ShowWindow(child.hwnd, SW_SHOWNA);
        }
        self.availability();
    }
    unsafe fn availability(&self) {
        let ready = self.pending.is_none();
        for id in [CHANNEL, CHECK] {
            EnableWindow(self.child(id), ready as i32);
        }
        EnableWindow(
            self.child(INSTALL),
            (ready && self.info.as_ref().is_some_and(|i| i.available)) as i32,
        );
        EnableWindow(
            self.child(ROLLBACK),
            (ready && self.previous.is_some()) as i32,
        );
    }
    unsafe fn status(&mut self, hwnd: HWND, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.error = error;
        SetWindowTextW(self.child(STATUS), wide(&self.status).as_ptr());
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn begin(&mut self, hwnd: HWND, operation: Operation) {
        if self.pending.is_some() {
            return;
        }
        let progress = match operation {
            Operation::Inspect => "正在检查本地上一版本备份…",
            Operation::Check(_) => "正在检查所选通道的 GitHub 正式发布版本…",
            Operation::Install(_) => "正在检查驱动兼容性、下载并校验更新；完成后会自动重启…",
            Operation::Rollback => "正在校验上一版本与已安装驱动；完成后会自动重启…",
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status(hwnd, progress, false);
        self.availability();
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = (|| match operation {
                Operation::Inspect => core_update::previous_version().map(Outcome::Inspected),
                Operation::Check(channel) => {
                    core_update::set_channel(&channel)?;
                    core_update::check(&channel).map(Outcome::Checked)
                }
                Operation::Install(info) => {
                    let prepared = core_update::prepare(&info)?;
                    core_update::launch(&prepared)?;
                    Ok(Outcome::Restart)
                }
                Operation::Rollback => {
                    let prepared = core_update::prepare_rollback()?;
                    core_update::launch(&prepared)?;
                    Ok(Outcome::Restart)
                }
            })();
            if tx.send(result).is_ok() {
                unsafe {
                    PostMessageW(window as HWND, DONE, 0, 0);
                }
            }
        });
    }
    unsafe fn finish(&mut self, hwnd: HWND) {
        let Some(receiver) = self.pending.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(Outcome::Inspected(previous))) => {
                self.previous = previous;
                let text = self
                    .previous
                    .as_ref()
                    .map(|v| format!("检测到可回退版本 {v}。点击检查更新查看当前发布。"))
                    .unwrap_or_else(|| "没有可回退的更早版本；安装新版本后会自动保留备份。".into());
                self.status(hwnd, text, false);
            }
            Ok(Ok(Outcome::Checked(info))) => {
                SetWindowTextW(
                    self.child(NOTES),
                    wide(if info.notes.is_empty() {
                        "发布方暂未提供更新内容。"
                    } else {
                        &info.notes
                    })
                    .as_ptr(),
                );
                let text = if info.available {
                    "发现新版本。点击“下载更新并重启”开始更新，准备失败时原程序继续运行。".into()
                } else {
                    format!(
                        "当前已是最新可用版本（仓库 {}），不会自动降级或重装。",
                        info.version
                    )
                };
                self.info = Some(info);
                self.status(hwnd, text, false);
            }
            Ok(Ok(Outcome::Restart)) => {
                self.restart = true;
                DestroyWindow(hwnd);
                return;
            }
            Ok(Err(error)) => {
                self.status(hwnd, format!("操作未完成：{error}"), true);
            }
            Err(error) => {
                self.status(hwnd, format!("后台任务未返回结果：{error}"), true);
            }
        }
        self.availability();
    }
    unsafe fn command(&mut self, hwnd: HWND, id: i32, event: u32) {
        if self.pending.is_some() {
            return;
        }
        match id {
            CHECK => {
                let index = SendMessageW(self.child(CHANNEL), CB_GETCURSEL, 0, 0);
                self.begin(
                    hwnd,
                    Operation::Check(if index == 1 { "preview" } else { "stable" }.into()),
                );
            }
            CHANNEL if event == CBN_SELCHANGE => {
                self.info = None;
                SetWindowTextW(
                    self.child(NOTES),
                    wide("通道已切换，点击检查更新读取该通道的实际版本。").as_ptr(),
                );
                self.status(hwnd, "通道选择仅影响主程序，设备驱动通道保持独立。", false);
                self.availability();
            }
            INSTALL => {
                if let Some(info) = self.info.as_ref().filter(|i| i.available) {
                    self.begin(hwnd, Operation::Install(info.clone()));
                }
            }
            ROLLBACK => {
                if self.previous.is_some() {
                    self.begin(hwnd, Operation::Rollback);
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
            c.text(
                "主程序更新",
                R::new(28, 18, 490, 34),
                23,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "核心与设备驱动分别更新，按需保持最新。",
                R::new(28, 56, 500, 24),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text("更新通道", R::new(548, 31, 88, 24), 13, 400, MUTED, DT_LEFT);
            c.round(R::new(24, 110, 892, 420), 14, PANEL, Some(BORDER));
            c.text(
                "当前版本",
                R::new(44, 134, 150, 26),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                env!("CARGO_PKG_VERSION"),
                R::new(210, 128, 450, 38),
                25,
                400,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "仓库版本",
                R::new(44, 184, 150, 26),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                self.info.as_ref().map_or("尚未检查", |i| &i.version),
                R::new(210, 178, 450, 38),
                25,
                400,
                if self.info.as_ref().is_some_and(|i| i.available) {
                    GREEN
                } else {
                    MUTED
                },
                DT_LEFT,
            );
            c.text(
                "下载大小",
                R::new(44, 234, 150, 26),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                &self
                    .info
                    .as_ref()
                    .map(|i| size(i.size))
                    .unwrap_or_else(|| "—".into()),
                R::new(210, 234, 650, 26),
                15,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text("更新内容", R::new(44, 272, 852, 23), 14, 600, TEXT, DT_LEFT);
            c.text(
                "仅替换主程序与文档；更新前备份旧 EXE 和配置，保留设备驱动及用户数据。",
                R::new(28, 598, 884, 22),
                13,
                400,
                DIM,
                DT_LEFT,
            );
            c.text(
                if self.pending.is_some() {
                    "后台处理中 · 完成前请勿关闭"
                } else if self.error {
                    "具体原因（可滚动查看）"
                } else {
                    "操作结果"
                },
                R::new(28, 620, 884, 18),
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
        let background = BG;
        let brush = CreateSolidBrush(
            ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
        );
        FillRect(item.hDC, &item.rcItem, brush);
        DeleteObject(brush);
        let c = Canvas::new(item.hDC, self.dpi);
        let width = (item.rcItem.right - item.rcItem.left) * 96 / self.dpi as i32;
        let height = (item.rcItem.bottom - item.rcItem.top) * 96 / self.dpi as i32;
        let disabled = item.itemState & ODS_DISABLED != 0;
        let primary = child.id == INSTALL;
        c.rect(R::new(0, 0, width, height), BG);
        c.round(
            R::new(0, 0, width - 1, height - 1),
            9,
            if disabled || !primary {
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
            if primary { 600 } else { 400 },
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
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Updater;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let updater = &mut *ptr;
    match message {
        WM_CREATE => {
            updater.layout(hwnd);
            updater.begin(hwnd, Operation::Inspect);
            0
        }
        WM_PAINT => {
            updater.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DRAWITEM => {
            updater.button(&*(l as *const DRAWITEMSTRUCT));
            1
        }
        WM_COMMAND => {
            updater.command(hwnd, (w & 0xffff) as i32, ((w >> 16) & 0xffff) as u32);
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        DONE => {
            updater.finish(hwnd);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
            SetTextColor(w as HDC, rgb(TEXT));
            SetBkColor(w as HDC, rgb(FIELD));
            updater.brush as isize
        }
        WM_CLOSE => {
            if updater.pending.is_some() {
                updater.status(
                    hwnd,
                    "更新操作尚未结束，请等待下载、校验或助手准备完成。",
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
/// Returns true only after the verified helper is ready and the main app must exit.
pub unsafe fn open(owner: HWND) -> bool {
    let dpi = GetDpiForWindow(owner).max(96);
    let instance = GetModuleHandleW(null());
    let class_name = wide("PeripheralDeskCoreUpdate");
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
    let mut updater = Box::new(Updater {
        dpi,
        font,
        brush: CreateSolidBrush(rgb(FIELD)),
        children: Vec::new(),
        info: None,
        previous: None,
        pending: None,
        status: "正在准备主程序更新…".into(),
        error: false,
        restart: false,
    });
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: updater.px(WIDTH),
        bottom: updater.px(HEIGHT),
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
        wide("Peripheral Desk · 主程序更新").as_ptr(),
        style,
        x,
        y,
        frame.right - frame.left,
        frame.bottom - frame.top,
        owner,
        null_mut(),
        instance,
        updater.as_mut() as *mut Updater as _,
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
    updater.restart
}
