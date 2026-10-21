use crate::{
    backend::{self, Action, Snapshot},
    devices::{Kind, LIGHTS, REGISTRY},
    hid::wide,
    model::Tuning,
    paint::{self, Canvas, Images, R, *},
};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::*, Threading::*},
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const DONE: u32 = WM_APP + 1;
const TRAY: u32 = WM_APP + 2;
const OPEN: u32 = WM_APP + 3;
const BATTERY: u32 = WM_APP + 4;
const CHIP: i32 = 1;
const GHOST: i32 = 3;
const CARD: i32 = 4;
type WorkResult = (Snapshot, Result<(), String>);
enum WorkUpdate {
    Device(usize, backend::State),
    Finished(WorkResult),
}
fn drain_work_updates(rx: &Receiver<WorkUpdate>) -> (Vec<WorkUpdate>, bool) {
    let mut updates = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(update) => updates.push(update),
            Err(mpsc::TryRecvError::Empty) => return (updates, false),
            Err(mpsc::TryRecvError::Disconnected) => return (updates, true),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Home,
    Device(usize),
    Preferences,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Home,
    Device(usize),
    Refresh,
    Close,
    Minimize,
    Quit,
    Prefs,
    Setting(Action),
    Step(i32),
    AudioStep(Tuning, i16),
    AudioPage(u8),
    ConnectAudio,
    ControlsEditor,
    ResidentMacros,
    DriverManager,
    AddDevice,
    Configuration,
    CoreUpdate,
    PresetLibrary,
    HeadsetLighting,
    GenericLighting,
    ExportAudio,
    ImportAudio,
    FlatEq,
    Interval(u32),
    Threshold(u8),
    Notices,
    Diagnose,
    Advanced(bool),
    SettingsPage(i32),
    HomePage(i32),
}
struct Button {
    hwnd: HWND,
    command: Command,
    label: String,
    rect: R,
    style: i32,
}
struct App {
    snapshot: Snapshot,
    page: Page,
    advanced: bool,
    settings_page: usize,
    audio_page: u8,
    eq_drag: Option<(Tuning, R)>,
    home_page: usize,
    controls: Vec<Button>,
    pending: Option<Receiver<WorkUpdate>>,
    modal_open: bool,
    queued: std::collections::VecDeque<(usize, Action)>,
    inflight: Option<(usize, Action)>,
    profile_notice: Option<&'static str>,
    preview_dpi: Option<u16>,
    dpi: u32,
    width: i32,
    height: i32,
    slider: R,
    dragging: bool,
    status: String,
    status_error: bool,
    images: Images,
    icon: HICON,
    tray_added: bool,
    taskbar_created: u32,
    flyout: HWND,
    main: HWND,
}

pub fn message(text: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("Peripheral Desk").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
pub fn run(hidden: bool, battery: bool) {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(null(), 0, wide("Local\\PeripheralDesk").as_ptr());
        if mutex.is_null() {
            message("无法创建程序实例锁");
            return;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let window = FindWindowW(wide("PeripheralDeskWindow").as_ptr(), null());
            if !window.is_null() {
                PostMessageW(window, if battery { BATTERY } else { OPEN }, 0, 0);
            }
            CloseHandle(mutex);
            return;
        }
        let images = Images::load();
        let icon = paint::tray_icon();
        let instance = GetModuleHandleW(null());
        for (name, proc) in [
            (
                "PeripheralDeskWindow",
                Some(window_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
            (
                "PeripheralDeskBattery",
                Some(flyout_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
        ] {
            let class = wide(name);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hIcon: icon,
                lpszClassName: class.as_ptr(),
                ..zeroed()
            };
            RegisterClassExW(&wc);
        }
        let mut app = Box::new(App {
            snapshot: Snapshot::load(),
            page: Page::Home,
            advanced: false,
            settings_page: 0,
            audio_page: 0,
            eq_drag: None,
            home_page: 0,
            controls: vec![],
            pending: None,
            modal_open: false,
            queued: std::collections::VecDeque::new(),
            inflight: None,
            profile_notice: None,
            preview_dpi: None,
            dpi: GetDpiForSystem(),
            width: 1240,
            height: 860,
            slider: R::default(),
            dragging: false,
            status: if REGISTRY.is_empty() {
                "打开驱动仓库，添加所需设备驱动".into()
            } else {
                "正在读取设备…".into()
            },
            status_error: false,
            images,
            icon,
            tray_added: false,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            flyout: null_mut(),
            main: null_mut(),
        });
        let dpi = app.dpi;
        let raw = &mut *app as *mut App;
        let hwnd = CreateWindowExW(
            0,
            wide("PeripheralDeskWindow").as_ptr(),
            wide("Peripheral Desk · 外设控制中心").as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (1240 * dpi / 96) as i32,
            (860 * dpi / 96) as i32,
            null_mut(),
            null_mut(),
            instance,
            raw as _,
        );
        if hwnd.is_null() {
            CloseHandle(mutex);
            message("无法创建窗口");
            return;
        }
        app.main = hwnd;
        let dark = 1i32;
        DwmSetWindowAttribute(hwnd, 20, &dark as *const _ as _, 4);
        let corner = 2i32;
        DwmSetWindowAttribute(hwnd, 33, &corner as *const _ as _, 4);
        if !hidden && !battery {
            ShowWindow(hwnd, SW_SHOW);
        }
        if battery {
            PostMessageW(hwnd, BATTERY, 0, 0);
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_KEYDOWN
                && msg.wParam == b'B' as usize
                && GetKeyState(VK_CONTROL as i32) < 0
            {
                SendMessageW(hwnd, BATTERY, 0, 0);
                continue;
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        DestroyIcon(icon);
        CloseHandle(mutex);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let a = &mut *ptr;
    if msg == a.taskbar_created {
        a.tray_added = false;
        a.tray(hwnd);
        return 0;
    }
    match msg {
        WM_CREATE => {
            a.main = hwnd;
            a.dpi = GetDpiForWindow(hwnd);
            a.layout(hwnd);
            SetTimer(hwnd, 1, a.snapshot.interval * 1000, None);
            a.tray(hwnd);
            a.begin(hwnd, None);
            0
        }
        WM_SIZE => {
            a.layout(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(l as *mut MINMAXINFO);
            m.ptMinTrackSize.x = a.px(1080);
            m.ptMinTrackSize.y = a.px(820);
            0
        }
        WM_DPICHANGED => {
            a.dpi = (w & 65535) as u32;
            let r = &*(l as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            a.layout(hwnd);
            0
        }
        WM_NCHITTEST => {
            let original = DefWindowProcW(hwnd, msg, w, l);
            if original != HTCLIENT as isize {
                return original;
            }
            let mut p = POINT {
                x: l as i16 as i32,
                y: (l >> 16) as i16 as i32,
            };
            ScreenToClient(hwnd, &mut p);
            let x = a.logical(p.x);
            let y = a.logical(p.y);
            if y < 62 && !a.controls.iter().any(|b| b.rect.contains(x, y)) {
                HTCAPTION as isize
            } else {
                HTCLIENT as isize
            }
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            a.paint(hwnd);
            0
        }
        WM_DRAWITEM => {
            a.draw_button(&*(l as *const DRAWITEMSTRUCT));
            1
        }
        WM_COMMAND => {
            if (w >> 16) & 65535 == BN_CLICKED as usize {
                if let Some(c) = a
                    .controls
                    .iter()
                    .find(|b| b.hwnd == l as HWND)
                    .map(|b| b.command)
                {
                    a.command(hwnd, c);
                }
            }
            0
        }
        WM_TIMER if w == 1 => {
            if a.queued.is_empty() {
                a.begin(hwnd, None);
            }
            0
        }
        WM_TIMER if w == 2 => {
            if a.pending.is_none() && !a.modal_open && !a.dragging && a.eq_drag.is_none() {
                KillTimer(hwnd, 2);
                if let Some(action) = a.queued.pop_front() {
                    a.begin(hwnd, Some(action));
                }
            }
            0
        }
        DONE => {
            a.complete(hwnd);
            0
        }
        OPEN => {
            let page = if w > 0 && w <= REGISTRY.len() {
                Page::Device(w - 1)
            } else {
                Page::Home
            };
            a.navigate(hwnd, page);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            0
        }
        BATTERY => {
            a.show_battery(hwnd);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = a.mouse(l);
            if let Some((key, rect)) = a.eq_hit(x, y) {
                a.eq_drag = Some((key, rect));
                SetCapture(hwnd);
                a.eq_slide(hwnd, key, rect, y);
            } else if a.slider.contains(x, y) && a.can_adjust() {
                a.dragging = true;
                SetCapture(hwnd);
                a.slide(hwnd, x);
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some((key, rect)) = a.eq_drag {
                let (_, y) = a.mouse(l);
                a.eq_slide(hwnd, key, rect, y);
            }
            if a.dragging {
                let (x, _) = a.mouse(l);
                a.slide(hwnd, x);
            }
            0
        }
        WM_LBUTTONUP => {
            if a.dragging || a.eq_drag.is_some() {
                a.dragging = false;
                a.eq_drag = None;
                ReleaseCapture();
            }
            0
        }
        WM_CAPTURECHANGED => {
            a.dragging = false;
            a.eq_drag = None;
            0
        }
        WM_MOUSEWHEEL => {
            if let Page::Device(m) = a.page {
                if a.can_adjust() && REGISTRY[m].kind == Kind::Mouse {
                    let mut p = POINT {
                        x: l as i16 as i32,
                        y: (l >> 16) as i16 as i32,
                    };
                    ScreenToClient(hwnd, &mut p);
                    if a.slider.contains(a.logical(p.x), a.logical(p.y)) {
                        a.step(hwnd, if (w >> 16) as i16 > 0 { 50 } else { -50 });
                    }
                }
            }
            0
        }
        WM_KEYDOWN if w == b'B' as usize && GetKeyState(VK_CONTROL as i32) < 0 => {
            a.show_battery(hwnd);
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE as usize => {
            if !a.modal_open {
                a.navigate(hwnd, Page::Home);
            }
            0
        }
        TRAY => {
            match l as u32 {
                WM_LBUTTONUP => a.show_battery(hwnd),
                WM_RBUTTONUP => a.menu(hwnd),
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            if !a.queued.is_empty() {
                a.set_status("待保存的设置将在后台完成", false);
            }
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            KillTimer(hwnd, 2);
            if !a.flyout.is_null() {
                DestroyWindow(a.flyout);
            }
            let n = tray_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &n);
            if let Err(e) = a.snapshot.save() {
                message(&format!("保存偏好失败：{e}"));
            }
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, w, l)
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

impl App {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    fn logical(&self, v: i32) -> i32 {
        v * 96 / self.dpi as i32
    }
    fn mouse(&self, l: LPARAM) -> (i32, i32) {
        (
            self.logical(l as i16 as i32),
            self.logical((l >> 16) as i16 as i32),
        )
    }
    fn set_status(&mut self, text: &str, error: bool) {
        self.status = text.into();
        self.status_error = error;
    }
    unsafe fn add(&mut self, hwnd: HWND, label: &str, r: R, style: i32, command: Command) {
        let h = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD | WS_TABSTOP | BS_OWNERDRAW as u32,
            self.px(r.x),
            self.px(r.y),
            self.px(r.w),
            self.px(r.h),
            hwnd,
            (1000 + self.controls.len()) as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SetWindowSubclass(h, Some(button_proc), 1, 0);
        self.controls.push(Button {
            hwnd: h,
            command,
            label: label.into(),
            rect: r,
            style,
        });
    }
    unsafe fn layout(&mut self, hwnd: HWND) {
        // Controls are created hidden and shown only after their owner-draw
        // metadata exists. Preserve keyboard focus across background refreshes.
        let focus = GetFocus();
        let focused = if GetForegroundWindow() == hwnd {
            self.controls
                .iter()
                .find(|b| b.hwnd == focus)
                .map(|b| b.command)
        } else {
            None
        };
        let mut r = zeroed();
        GetClientRect(hwnd, &mut r);
        self.width = self.logical(r.right).max(1000);
        self.height = self.logical(r.bottom).max(780);
        for b in self.controls.drain(..) {
            DestroyWindow(b.hwnd);
        }
        self.slider = R::default();
        if self.eq_drag.is_some() {
            ReleaseCapture();
        }
        self.eq_drag = None;
        if let Page::Device(m) = self.page {
            self.settings_page = self.settings_page.min(self.settings_pages(m) - 1);
        }
        let w = self.width;
        let h = self.height;
        self.add(
            hwnd,
            "—",
            R::new(w - 98, 15, 34, 32),
            GHOST,
            Command::Minimize,
        );
        self.add(hwnd, "×", R::new(w - 56, 15, 34, 32), GHOST, Command::Close);
        match self.page {
            Page::Home => {
                self.add(
                    hwnd,
                    "检查更新",
                    R::new(w - 558, 102, 116, 38),
                    GHOST,
                    Command::CoreUpdate,
                );
                self.add(
                    hwnd,
                    "配置备份",
                    R::new(w - 432, 102, 116, 38),
                    GHOST,
                    Command::Configuration,
                );
                self.add(
                    hwnd,
                    "驱动仓库",
                    R::new(w - 306, 102, 146, 38),
                    GHOST,
                    Command::DriverManager,
                );
                self.add(
                    hwnd,
                    "重新读取",
                    R::new(w - 154, 102, 122, 40),
                    GHOST,
                    Command::Refresh,
                );
                if REGISTRY.is_empty() {
                    let center_y = 176 + (h - 266) / 2;
                    self.add(
                        hwnd,
                        "＋  添加设备驱动",
                        R::new(w / 2 - 132, center_y + 8, 264, 48),
                        CHIP,
                        Command::AddDevice,
                    );
                }
                let cols = if REGISTRY.len() > 3 {
                    2
                } else {
                    (REGISTRY.len() as i32).clamp(1, 3)
                };
                let page_size = if REGISTRY.len() > 3 { 4 } else { 3 };
                let cw = (w - 64 - 20 * (cols - 1)) / cols;
                let card_h = if REGISTRY.len() > 3 {
                    (h - 250) / 2
                } else {
                    (h - 275).min(530)
                };
                for (slot, (index, spec)) in REGISTRY
                    .iter()
                    .enumerate()
                    .skip(self.home_page * page_size)
                    .take(page_size)
                    .enumerate()
                {
                    let x = 32 + (slot as i32 % cols) * (cw + 20);
                    let y = 176 + slot as i32 / cols * (card_h + 16);
                    self.add(
                        hwnd,
                        spec.name,
                        R::new(x, y, cw, card_h),
                        CARD,
                        Command::Device(index),
                    );
                }
                if REGISTRY.len() > page_size {
                    self.add(
                        hwnd,
                        "上一页",
                        R::new(w / 2 - 110, h - 62, 90, 38),
                        GHOST,
                        Command::HomePage(-1),
                    );
                    self.add(
                        hwnd,
                        "下一页",
                        R::new(w / 2 + 20, h - 62, 90, 38),
                        GHOST,
                        Command::HomePage(1),
                    );
                }
                self.add(
                    hwnd,
                    "工具设置",
                    R::new(32, h - 62, 112, 38),
                    GHOST,
                    Command::Prefs,
                );
                self.add(
                    hwnd,
                    "退出程序",
                    R::new(w - 144, h - 62, 112, 38),
                    GHOST,
                    Command::Quit,
                );
            }
            Page::Device(m) => {
                self.add(
                    hwnd,
                    "驱动管理",
                    R::new(174, 16, 112, 34),
                    GHOST,
                    Command::DriverManager,
                );
                self.add(
                    hwnd,
                    "配置备份",
                    R::new(
                        if REGISTRY[m].kind == Kind::Headset {
                            412
                        } else {
                            28
                        },
                        h - 62,
                        136,
                        38,
                    ),
                    GHOST,
                    Command::Configuration,
                );
                self.add(
                    hwnd,
                    "‹  我的设备",
                    R::new(28, 16, 133, 34),
                    GHOST,
                    Command::Home,
                );
                let rx = 394;
                let rw = w - rx - 28;
                let cx = rx + 30;
                if REGISTRY[m].kind == Kind::Headset {
                    for (i, label) in crate::audio_fx::PAGES.iter().enumerate() {
                        self.add(
                            hwnd,
                            label,
                            R::new(rx + i as i32 * 86, 17, 80, 34),
                            GHOST,
                            Command::AudioPage(i as u8),
                        );
                    }
                    self.add(
                        hwnd,
                        "EQ 预设",
                        R::new(rx + 444, 17, 96, 34),
                        GHOST,
                        Command::PresetLibrary,
                    );
                    self.add(
                        hwnd,
                        "灯光",
                        R::new(294, 17, 76, 34),
                        GHOST,
                        Command::HeadsetLighting,
                    );
                    self.add(
                        hwnd,
                        "连接音效组件",
                        R::new(28, h - 62, 148, 38),
                        GHOST,
                        Command::ConnectAudio,
                    );
                    self.add(
                        hwnd,
                        "导出音效",
                        R::new(188, h - 62, 96, 38),
                        GHOST,
                        Command::ExportAudio,
                    );
                    self.add(
                        hwnd,
                        "导入音效",
                        R::new(296, h - 62, 96, 38),
                        GHOST,
                        Command::ImportAudio,
                    );
                }
                if REGISTRY[m].kind != Kind::Headset && !REGISTRY[m].advanced.is_empty() {
                    self.add(
                        hwnd,
                        "基础设置",
                        R::new(rx + 10, 17, 112, 34),
                        GHOST,
                        Command::Advanced(false),
                    );
                    self.add(
                        hwnd,
                        "高级设置",
                        R::new(rx + 132, 17, 112, 34),
                        GHOST,
                        Command::Advanced(true),
                    );
                }
                if REGISTRY[m].supports("lighting-editor") {
                    self.add(
                        hwnd,
                        "灯效设置",
                        R::new(294, 17, 96, 34),
                        GHOST,
                        Command::GenericLighting,
                    );
                }
                if REGISTRY[m].supports("software-macro-trigger")
                    || matches!(
                        REGISTRY[m].key,
                        "razer-deathadder-v3-pro" | "mchose-a5-ultra" | "mchose-g75-pro"
                    )
                {
                    self.add(
                        hwnd,
                        "软件宏",
                        R::new(rx + 442, 17, 112, 34),
                        GHOST,
                        Command::ResidentMacros,
                    );
                }
                if REGISTRY[m].supports("bindings") || REGISTRY[m].supports("key-colors") {
                    self.add(
                        hwnd,
                        "按键与板载设置",
                        R::new(rx + 254, 17, 178, 34),
                        GHOST,
                        Command::ControlsEditor,
                    );
                }
                if (self.advanced || REGISTRY[m].kind == Kind::Headset)
                    && !REGISTRY[m].advanced.is_empty()
                {
                    if REGISTRY[m].kind != Kind::Headset && self.settings_pages(m) > 1 {
                        self.add(
                            hwnd,
                            "上一页",
                            R::new(rx + rw - 240, 101, 64, 34),
                            GHOST,
                            Command::SettingsPage(-1),
                        );
                        self.add(
                            hwnd,
                            "下一页",
                            R::new(rx + rw - 94, 101, 64, 34),
                            GHOST,
                            Command::SettingsPage(1),
                        );
                    }
                    for (row, setting) in self.visible_settings(m).iter().enumerate() {
                        let y = 172 + row as i32 * 78;
                        let volume = is_volume(setting.key);
                        let slots = setting.choices.len() as i32 + if volume { 2 } else { 0 };
                        let bw = ((rw - 280 - 6 * (slots - 1)) / slots).clamp(28, 88);
                        let x = rx + rw - 30 - (bw + 6) * slots + 6;
                        if volume {
                            self.add(
                                hwnd,
                                "−",
                                R::new(x, y, bw, 35),
                                CHIP,
                                Command::AudioStep(setting.key, -1),
                            );
                            self.add(
                                hwnd,
                                "+",
                                R::new(x + (slots - 1) * (bw + 6), y, bw, 35),
                                CHIP,
                                Command::AudioStep(setting.key, 1),
                            );
                        }
                        for (i, (value, label)) in setting.choices.iter().enumerate() {
                            self.add(
                                hwnd,
                                label,
                                R::new(x + (i as i32 + i32::from(volume)) * (bw + 6), y, bw, 35),
                                CHIP,
                                Command::Setting(Action::Tune(setting.key, *value)),
                            );
                        }
                    }
                    if REGISTRY[m].kind == Kind::Headset && self.audio_page == 1 {
                        let bw = (rw - 72) / 10;
                        for (i, (key, _)) in crate::audio_fx::EQ_BANDS.iter().enumerate() {
                            let x = cx + 6 + i as i32 * bw;
                            self.add(
                                hwnd,
                                "+",
                                R::new(x, 272, bw - 10, 30),
                                CHIP,
                                Command::AudioStep(*key, 1),
                            );
                            self.add(
                                hwnd,
                                "−",
                                R::new(x, 522, bw - 10, 30),
                                CHIP,
                                Command::AudioStep(*key, -1),
                            );
                        }
                        self.add(
                            hwnd,
                            "所有频段归零",
                            R::new(cx, 586, 156, 36),
                            GHOST,
                            Command::FlatEq,
                        );
                    }
                } else if let Some((min, max)) = REGISTRY[m].dpi_range {
                    let stages = self.snapshot.devices[m].dpi_stages.clone();
                    let count = if stages.is_empty() { 5 } else { stages.len() };
                    let bw = (rw - 60 - 10 * (count as i32 - 1)) / count as i32;
                    for i in 0..count {
                        let (label, action) = if stages.is_empty() {
                            let v = [400, 800, 1200, 1600, 3200][i];
                            (format!("{v}"), Action::Dpi(v))
                        } else {
                            (format!("{} · {}", i + 1, stages[i]), Action::Stage(i as u8))
                        };
                        self.add(
                            hwnd,
                            &label,
                            R::new(cx + i as i32 * (bw + 10), 178, bw, 38),
                            CHIP,
                            Command::Setting(action),
                        );
                    }
                    self.slider = R::new(cx, 239, rw - 158, 24);
                    self.add(
                        hwnd,
                        "−",
                        R::new(rx + rw - 112, 232, 34, 34),
                        GHOST,
                        Command::Step(-50),
                    );
                    self.add(
                        hwnd,
                        "+",
                        R::new(rx + rw - 68, 232, 34, 34),
                        GHOST,
                        Command::Step(50),
                    );
                    let _ = (min, max);
                } else if REGISTRY[m].kind == Kind::Keyboard && REGISTRY[m].supports("settings") {
                    let bw = (rw - 60 - 40) / 5;
                    for (i, (value, label)) in LIGHTS.iter().enumerate() {
                        self.add(
                            hwnd,
                            label,
                            R::new(
                                cx + (i % 5) as i32 * (bw + 10),
                                172 + (i / 5) as i32 * 48,
                                bw,
                                36,
                            ),
                            CHIP,
                            Command::Setting(Action::Light(*value)),
                        );
                    }
                }
                if REGISTRY[m].kind != Kind::Headset
                    && (!self.advanced || REGISTRY[m].advanced.is_empty())
                {
                    let rates = REGISTRY[m].rates_for(self.snapshot.devices[m].wired);
                    let gap = 8;
                    let bw = ((rw - 200 - gap * (rates.len() as i32 - 1))
                        / rates.len().max(1) as i32)
                        .min(100);
                    let x = rx + rw - 30 - (bw + gap) * rates.len() as i32 + gap;
                    for (i, rate) in rates.iter().enumerate() {
                        self.add(
                            hwnd,
                            &format!("{rate} Hz"),
                            R::new(x + i as i32 * (bw + gap), 358, bw, 37),
                            CHIP,
                            Command::Setting(Action::Rate(*rate)),
                        );
                    }
                    let sleeps = REGISTRY[m].sleeps;
                    let zero_extra = if sleeps.contains(&0) { 18 } else { 0 };
                    let bw = ((rw - 200 - 6 * (sleeps.len() as i32 - 1) - zero_extra)
                        / sleeps.len().max(1) as i32)
                        .min(85);
                    let mut x = rx + rw - 30 - ((bw + 6) * sleeps.len() as i32 - 6 + zero_extra);
                    for sec in sleeps {
                        let width = bw + if *sec == 0 { zero_extra } else { 0 };
                        self.add(
                            hwnd,
                            &sleep_label(*sec, false),
                            R::new(x, 425, width, 35),
                            CHIP,
                            Command::Setting(Action::Sleep(*sec)),
                        );
                        x += width + 6;
                    }
                }
                self.add(
                    hwnd,
                    "重新读取",
                    R::new(w - 150, h - 61, 122, 39),
                    GHOST,
                    Command::Refresh,
                );
                self.add(
                    hwnd,
                    "设备诊断",
                    R::new(w - 280, h - 61, 118, 39),
                    GHOST,
                    Command::Diagnose,
                );
            }
            Page::Preferences => {
                self.add(
                    hwnd,
                    "检查更新",
                    R::new(330, 566, 180, 40),
                    GHOST,
                    Command::CoreUpdate,
                );
                self.add(
                    hwnd,
                    "配置备份与恢复",
                    R::new(526, 566, 180, 40),
                    GHOST,
                    Command::Configuration,
                );
                self.add(
                    hwnd,
                    "‹  我的设备",
                    R::new(28, 16, 133, 34),
                    GHOST,
                    Command::Home,
                );
                for (i, value) in [30, 60, 120, 300, 600, 1800].iter().enumerate() {
                    self.add(
                        hwnd,
                        &format!("{value} 秒"),
                        R::new(330 + i as i32 * 116, 214, 104, 40),
                        CHIP,
                        Command::Interval(*value),
                    );
                }
                for (i, value) in [5, 10, 15, 20, 25, 30, 40, 50].iter().enumerate() {
                    self.add(
                        hwnd,
                        &format!("{value}%"),
                        R::new(330 + i as i32 * 84, 307, 72, 40),
                        CHIP,
                        Command::Threshold(*value),
                    );
                }
                self.add(
                    hwnd,
                    "启用低电提醒",
                    R::new(330, 400, 180, 40),
                    CHIP,
                    Command::Notices,
                );
                self.add(
                    hwnd,
                    "导出设备诊断",
                    R::new(330, 492, 180, 40),
                    GHOST,
                    Command::Diagnose,
                );
            }
        }
        self.render(hwnd);
        for b in &self.controls {
            ShowWindow(b.hwnd, SW_SHOWNA);
        }
        if let Some(command) = focused {
            let target = self
                .controls
                .iter()
                .find(|b| b.command == command && IsWindowEnabled(b.hwnd) != 0)
                .or_else(|| {
                    self.controls.iter().find(|b| {
                        IsWindowEnabled(b.hwnd) != 0
                            && !matches!(b.command, Command::Close | Command::Minimize)
                    })
                });
            if let Some(b) = target {
                SetFocus(b.hwnd);
            }
        }
    }
    unsafe fn navigate(&mut self, hwnd: HWND, page: Page) {
        self.page = page;
        self.advanced = false;
        self.settings_page = 0;
        self.audio_page = 0;
        self.preview_dpi = None;
        self.layout(hwnd);
        InvalidateRect(hwnd, null(), 0);
    }
    fn can_adjust(&self) -> bool {
        matches!(self.page,Page::Device(m)if self.snapshot.devices[m].fresh)
            && self.pending.is_none()
            && !self.modal_open
    }
    fn value(&self, m: usize) -> Option<u16> {
        self.queued
            .iter()
            .rev()
            .chain(self.inflight.iter())
            .find_map(|(id, a)| {
                if *id == m {
                    if let Action::Dpi(v) = a {
                        return Some(*v);
                    }
                }
                None
            })
            .or(self.snapshot.devices[m].dpi)
    }
    unsafe fn schedule(&mut self, hwnd: HWND, action: Action) {
        if let Page::Device(m) = self.page {
            if !self.can_adjust() {
                return;
            }
            if let Action::Dpi(v) = action {
                self.preview_dpi = Some(v);
            }
            coalesce(&mut self.queued, m, action);
            SetTimer(hwnd, 2, 1500, None);
            self.set_status("调整将在停止操作 1.5 秒后保存", false);
            self.render(hwnd);
        }
    }
    unsafe fn slide(&mut self, hwnd: HWND, x: i32) {
        if let Page::Device(m) = self.page {
            if let Some((min, max)) = REGISTRY[m].dpi_range {
                let ratio = ((x - self.slider.x) as f32 / self.slider.w as f32).clamp(0., 1.);
                let step = REGISTRY[m].dpi_step.max(1);
                let value = (((min as f32 + (max - min) as f32 * ratio) / step as f32).round()
                    as u32
                    * step as u32)
                    .min(max as u32) as u16;
                self.schedule(hwnd, Action::Dpi(value.clamp(min, max)));
            }
        }
    }
    unsafe fn step(&mut self, hwnd: HWND, delta: i32) {
        if let Page::Device(m) = self.page {
            if let (Some((min, max)), Some(value)) = (REGISTRY[m].dpi_range, self.value(m)) {
                self.schedule(
                    hwnd,
                    Action::Dpi(
                        (value as i32 + delta.signum() * REGISTRY[m].dpi_step as i32)
                            .clamp(min as i32, max as i32) as u16,
                    ),
                );
            }
        }
    }
    unsafe fn command(&mut self, hwnd: HWND, command: Command) {
        match command {
            Command::AudioPage(value) => {
                self.audio_page = value;
                self.layout(hwnd);
            }
            Command::Configuration | Command::CoreUpdate => {
                if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
                    return;
                }
                self.modal_open = true;
                let restarting = if command == Command::CoreUpdate {
                    crate::update_ui::open(hwnd)
                } else {
                    let model = if let Page::Device(m) = self.page {
                        Some(m)
                    } else {
                        None
                    };
                    crate::configuration_ui::open(hwnd, model);
                    false
                };
                self.modal_open = false;
                if restarting {
                    DestroyWindow(hwnd);
                    return;
                }
                self.images = Images::load();
                self.layout(hwnd);
                self.begin(hwnd, None);
            }
            Command::DriverManager | Command::AddDevice => {
                if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
                    return;
                }
                self.modal_open = true;
                crate::plugin_ui::open(hwnd);
                self.modal_open = false;
                self.images = Images::load();
                self.layout(hwnd);
                self.begin(hwnd, None);
            }
            Command::ControlsEditor
            | Command::ResidentMacros
            | Command::PresetLibrary
            | Command::HeadsetLighting
            | Command::GenericLighting => {
                if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
                    return;
                }
                if let Page::Device(model) = self.page {
                    self.modal_open = true;
                    match command {
                        Command::ControlsEditor => crate::controls_ui::open(hwnd, model),
                        Command::ResidentMacros => crate::resident_macro_ui::open(hwnd, model),
                        Command::PresetLibrary => crate::preset_ui::open(hwnd, model),
                        Command::GenericLighting => crate::generic_lighting_ui::open(hwnd, model),
                        _ => crate::lighting_ui::open(hwnd),
                    }
                    self.modal_open = false;
                    self.begin(hwnd, None);
                }
            }
            Command::ConnectAudio => self.connect_audio(hwnd),
            Command::ExportAudio => self.audio_profile(hwnd, false),
            Command::ImportAudio => self.audio_profile(hwnd, true),
            Command::FlatEq => {
                if let Page::Device(m) = self.page {
                    for (key, _) in crate::audio_fx::EQ_BANDS {
                        if self.tuning_value(m, *key).is_some() {
                            self.schedule(hwnd, Action::Tune(*key, 20));
                        }
                    }
                }
            }
            Command::SettingsPage(delta) => {
                if let Page::Device(m) = self.page {
                    self.settings_page = (self.settings_page as i32 + delta)
                        .clamp(0, self.settings_pages(m) as i32 - 1)
                        as usize;
                    self.layout(hwnd);
                }
            }
            Command::Advanced(value) => {
                self.advanced = value;
                self.settings_page = 0;
                self.layout(hwnd);
            }
            Command::HomePage(delta) => {
                self.home_page = (self.home_page as i32 + delta).clamp(
                    0,
                    (REGISTRY
                        .len()
                        .div_ceil(if REGISTRY.len() > 3 { 4 } else { 3 })
                        - 1) as i32,
                ) as usize;
                self.layout(hwnd);
            }
            Command::Home => self.navigate(hwnd, Page::Home),
            Command::Device(m) => self.navigate(hwnd, Page::Device(m)),
            Command::Prefs => self.navigate(hwnd, Page::Preferences),
            Command::Minimize => {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
            Command::Close => {
                ShowWindow(hwnd, SW_HIDE);
            }
            Command::Quit => self.quit(hwnd),
            Command::Refresh => self.begin(hwnd, None),
            Command::Setting(a) => self.schedule(hwnd, a),
            Command::Step(n) => self.step(hwnd, n),
            Command::AudioStep(key, delta) => {
                if let Page::Device(m) = self.page {
                    if let Some(value) = self.tuning_value(m, key) {
                        self.schedule(
                            hwnd,
                            Action::Tune(
                                key,
                                (value as i16 + delta).clamp(
                                    0,
                                    crate::audio_fx::spec(key).map_or(100, |s| s.max) as i16,
                                ) as u16,
                            ),
                        );
                    }
                }
            }
            Command::Interval(v) => {
                self.snapshot.interval = v;
                SetTimer(hwnd, 1, v * 1000, None);
                self.save_preference(hwnd);
            }
            Command::Threshold(v) => {
                self.snapshot.threshold = v;
                self.save_preference(hwnd);
            }
            Command::Notices => {
                self.snapshot.notices = !self.snapshot.notices;
                self.save_preference(hwnd);
            }
            Command::Diagnose => {
                let path = backend::data_path().with_file_name("diagnostics.json");
                let key = if let Page::Device(m) = self.page {
                    Some(REGISTRY[m].key)
                } else {
                    None
                };
                match crate::configuration::export_diagnostics(key, &path) {
                    Ok(_) => message(&format!("已导出匿名诊断信息：\n{}", path.display())),
                    Err(e) => message(&format!("导出失败：{e}")),
                }
            }
        }
    }
    unsafe fn save_preference(&mut self, hwnd: HWND) {
        match self.snapshot.save() {
            Ok(()) => self.set_status("工具设置已保存", false),
            Err(e) => self.set_status(&format!("保存失败：{e}"), true),
        }
        self.render(hwnd);
    }
    unsafe fn quit(&self, hwnd: HWND) {
        if self.modal_open {
            return;
        }
        if self.inflight.is_some() || self.profile_notice.is_some() || !self.queued.is_empty() {
            message("设备设置正在保存，请等待完成后退出。");
        } else {
            DestroyWindow(hwnd);
        }
    }
    unsafe fn begin(&mut self, hwnd: HWND, action: Option<(usize, Action)>) {
        if self.pending.is_some() || self.modal_open {
            return;
        }
        let snapshot = self.snapshot.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.inflight = action;
        self.profile_notice = None;
        self.set_status(
            if action.is_some() {
                "正在保存到设备并读回确认…"
            } else if REGISTRY.is_empty() {
                "打开驱动仓库，添加所需设备驱动；安装后请重启主程序"
            } else {
                "正在读取设备…"
            },
            false,
        );
        self.render(hwnd);
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = if let Some(action) = action {
                backend::refresh(&snapshot, Some(action))
            } else {
                backend::refresh_progress(&snapshot, |model, state| {
                    if tx.send(WorkUpdate::Device(model, state)).is_ok() {
                        PostMessageW(window as HWND, DONE, 0, 0);
                    }
                })
            };
            if tx.send(WorkUpdate::Finished(result)).is_ok() {
                PostMessageW(window as HWND, DONE, 0, 0);
            }
        });
    }
    unsafe fn complete(&mut self, hwnd: HWND) {
        let Some(rx) = self.pending.as_ref() else {
            return;
        };
        let (updates, disconnected) = drain_work_updates(rx);
        if updates.is_empty() && !disconnected {
            return;
        }
        let mut finished = None;
        let mut changed = false;
        for update in updates {
            match update {
                WorkUpdate::Device(model, state) => {
                    self.snapshot.devices[model] = state;
                    changed = true;
                }
                WorkUpdate::Finished(result) => finished = Some(result),
            }
        }
        if finished.is_none() && disconnected {
            self.pending = None;
            self.inflight = None;
            self.profile_notice = None;
            self.set_status("后台读取失败：后台线程已退出", true);
        }
        if finished.is_none() && changed {
            self.tray(hwnd);
            self.layout(hwnd);
            if !self.flyout.is_null() {
                InvalidateRect(self.flyout, null(), 0);
            }
        }
        if finished.is_none() {
            if disconnected {
                self.render(hwnd);
                if !self.queued.is_empty() {
                    SetTimer(hwnd, 2, 100, None);
                }
            }
            return;
        }
        let Some(_rx) = self.pending.take() else {
            return;
        };
        let (s, result) = finished.unwrap();
        let writing = self.inflight.take().is_some();
        let profile_notice = self.profile_notice.take();
        let mut s = s;
        s.interval = self.snapshot.interval;
        s.threshold = self.snapshot.threshold;
        s.notices = self.snapshot.notices;
        self.snapshot = s;
        if self.queued.is_empty() {
            self.preview_dpi = None;
        }
        let alerts = self.snapshot.alerts();
        let save = self.snapshot.save();
        match (result, save) {
            (Err(e), _) => self.set_status(
                &format!(
                    "{}：{e}",
                    if profile_notice.is_some() {
                        "音效配置操作失败"
                    } else {
                        "未能确认保存"
                    }
                ),
                true,
            ),
            (_, Err(e)) => self.set_status(&format!("本地偏好保存失败：{e}"), true),
            _ => {
                let count = self.snapshot.devices.iter().filter(|d| d.fresh).count();
                self.set_status(
                    &if let Some(notice) = profile_notice {
                        notice.into()
                    } else if writing {
                        "设置已保存，读回确认成功".into()
                    } else if REGISTRY.is_empty() {
                        "打开驱动仓库添加设备；新驱动安装后请重启主程序".into()
                    } else {
                        format!("{count} 台设备在线 · 状态已更新")
                    },
                    false,
                );
            }
        }
        self.tray(hwnd);
        for i in alerts {
            self.notify(hwnd, i);
        }
        self.layout(hwnd);
        if !self.flyout.is_null() {
            InvalidateRect(self.flyout, null(), 0);
        }
        if !self.queued.is_empty() {
            SetTimer(hwnd, 2, 100, None);
        }
    }
    fn setting_rows(&self) -> usize {
        ((self.height - 351) / 78).max(1) as usize
    }
    fn settings_pages(&self, m: usize) -> usize {
        if REGISTRY[m].kind == Kind::Headset {
            1
        } else {
            REGISTRY[m]
                .advanced
                .len()
                .div_ceil(self.setting_rows())
                .max(1)
        }
    }
    fn visible_settings(&self, m: usize) -> Vec<&'static crate::devices::SettingSpec> {
        let audio = REGISTRY[m].kind == Kind::Headset;
        REGISTRY[m]
            .advanced
            .iter()
            .filter(|s| {
                REGISTRY[m].kind != Kind::Headset
                    || (crate::audio_fx::page(s.key) == self.audio_page
                        && !(self.audio_page == 1 && s.key != Tuning::EqEnabled))
            })
            .skip(if audio {
                0
            } else {
                self.settings_page * self.setting_rows()
            })
            .take(if audio {
                usize::MAX
            } else {
                self.setting_rows()
            })
            .collect()
    }
    fn eq_hit(&self, x: i32, y: i32) -> Option<(Tuning, R)> {
        if !self.can_adjust() || self.audio_page != 1 {
            return None;
        }
        let Page::Device(m) = self.page else {
            return None;
        };
        if REGISTRY[m].kind != Kind::Headset {
            return None;
        }
        let bw = (self.width - 394 - 28 - 72) / 10;
        crate::audio_fx::EQ_BANDS
            .iter()
            .enumerate()
            .find_map(|(i, (key, _))| {
                let rect = R::new(430 + i as i32 * bw, 314, bw - 10, 190);
                (rect.contains(x, y) && self.tuning_value(m, *key).is_some())
                    .then_some((*key, rect))
            })
    }
    unsafe fn eq_slide(&mut self, hwnd: HWND, key: Tuning, rect: R, y: i32) {
        let v = ((1. - ((y - rect.y) as f32 / rect.h as f32).clamp(0., 1.)) * 40.).round() as u16;
        self.schedule(hwnd, Action::Tune(key, v));
    }
    unsafe fn eq_panel(&self, c: &Canvas, m: usize) {
        let bw = (self.width - 394 - 28 - 72) / 10;
        for (i, (key, label)) in crate::audio_fx::EQ_BANDS.iter().enumerate() {
            let x = 430 + i as i32 * bw;
            let center = x + (bw - 10) / 2;
            let value = self.tuning_value(m, *key);
            let available = value.is_some() && self.snapshot.devices[m].fresh;
            c.line(center, 314, center, 504, BORDER, 5.);
            c.line(center - 7, 409, center + 7, 409, DIM, 1.);
            if let Some(v) = value {
                let y = 504 - (v as f32 / 40. * 190.).round() as i32;
                c.line(
                    center,
                    409,
                    center,
                    y,
                    if available { RED } else { DIM },
                    5.,
                );
                c.round(
                    R::new(center - 9, y - 6, 18, 12),
                    5,
                    if available { ORANGE } else { DIM },
                    None,
                );
            }
            c.text(
                &value
                    .map(|v| format!("{:+}", v as i16 - 20))
                    .unwrap_or_else(|| "—".into()),
                R::new(x - 5, 239, bw, 25),
                12,
                400,
                MUTED,
                DT_CENTER,
            );
            c.text(label, R::new(x - 5, 556, bw, 22), 12, 400, DIM, DT_CENTER);
        }
    }
    unsafe fn connect_audio(&mut self, hwnd: HWND) {
        use windows_sys::Win32::UI::Controls::Dialogs::*;
        if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
            return;
        }
        let mut file = [0u16; 32768];
        let filter = wide("C-Media 音效组件 (osConfLib.dll)\0osConfLib.dll\0\0");
        let mut dialog: OPENFILENAMEW = zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = hwnd;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
        let title = wide("选择官方安装目录中的 x64 / osConfLib.dll");
        dialog.lpstrTitle = title.as_ptr();
        self.modal_open = true;
        let accepted = GetOpenFileNameW(&mut dialog);
        self.modal_open = false;
        if accepted != 0 {
            let end = file.iter().position(|&v| v == 0).unwrap_or(file.len());
            let path = std::path::PathBuf::from(String::from_utf16_lossy(&file[..end]));
            match crate::audio_fx::configure(&path) {
                Ok(()) => {
                    self.set_status("音效组件已连接，正在读取…", false);
                    self.begin(hwnd, None);
                }
                Err(e) => self.set_status(&e, true),
            }
        } else {
            let error = CommDlgExtendedError();
            if error != 0 {
                self.set_status(&format!("选择音效组件失败：{error}"), true);
            }
        }
        self.render(hwnd);
    }
    unsafe fn audio_profile(&mut self, hwnd: HWND, import: bool) {
        let Page::Device(model) = self.page else {
            return;
        };
        use windows_sys::Win32::UI::Controls::Dialogs::*;
        if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
            return;
        }
        let mut file = [0u16; 32768];
        if !import {
            let name = wide("G2-Pro-effects.json");
            file[..name.len()].copy_from_slice(&name);
        }
        let filter = wide("G2 Pro 音效配置 (*.json)\0*.json\0\0");
        let extension = wide("json");
        let title = wide(if import {
            "导入 G2 Pro 音效配置"
        } else {
            "导出 G2 Pro 音效配置"
        });
        let mut dialog: OPENFILENAMEW = zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = hwnd;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.lpstrDefExt = extension.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_PATHMUSTEXIST
            | OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | if import {
                OFN_FILEMUSTEXIST
            } else {
                OFN_OVERWRITEPROMPT
            };
        self.modal_open = true;
        let accepted = if import {
            GetOpenFileNameW(&mut dialog)
        } else {
            GetSaveFileNameW(&mut dialog)
        };
        self.modal_open = false;
        if accepted == 0 {
            let error = CommDlgExtendedError();
            if error != 0 {
                self.set_status(&format!("选择配置文件失败：{error}"), true);
                self.render(hwnd);
            }
            return;
        }
        let end = file.iter().position(|&v| v == 0).unwrap_or(file.len());
        let path = std::path::PathBuf::from(String::from_utf16_lossy(&file[..end]));
        let snapshot = self.snapshot.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.inflight = None;
        self.profile_notice = Some(if import {
            "音效配置已导入，读回确认成功"
        } else {
            "音效配置已导出"
        });
        self.set_status(
            if import {
                "正在导入音效配置并读回确认…"
            } else {
                "正在读取并导出音效配置…"
            },
            false,
        );
        self.render(hwnd);
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = if import {
                crate::profiles::load_audio(&path)
                    .and_then(|values| crate::audio_fx::apply_profile(&values))
                    .map(|_| ())
            } else {
                crate::profiles::export_audio(&path)
            };
            let (state, _) = backend::refresh_device(&snapshot, model);
            if tx.send(WorkUpdate::Finished((state, result))).is_ok() {
                PostMessageW(window as HWND, DONE, 0, 0);
            }
        });
    }
    fn tuning_value(&self, m: usize, key: Tuning) -> Option<u16> {
        self.queued
            .iter()
            .rev()
            .copied()
            .chain(self.inflight)
            .find_map(|(id, a)| match a {
                Action::Tune(k, v) if id == m && k == key => Some(v),
                _ => None,
            })
            .or_else(|| self.snapshot.devices[m].tuning.get(&key).copied())
    }
    fn selected(&self, command: Command) -> bool {
        match command {
            Command::Setting(action) => {
                if let Page::Device(m) = self.page {
                    if let Some((id, queued)) = self
                        .queued
                        .iter()
                        .rev()
                        .copied()
                        .find(|(id, value)| *id == m && same_kind(*value, action))
                        .or(self.inflight)
                    {
                        if id == m && same_kind(queued, action) {
                            return queued == action;
                        }
                    }
                    let s = &self.snapshot.devices[m];
                    match action {
                        Action::Dpi(v) => self.value(m) == Some(v),
                        Action::Rate(v) => s.rate == Some(v),
                        Action::Sleep(v) => s.sleep_seconds == Some(v),
                        Action::Light(v) => s.light == Some(v),
                        Action::Stage(v) => s.active_stage == Some(v),
                        Action::Tune(key, v) => s.tuning.get(&key) == Some(&v),
                    }
                } else {
                    false
                }
            }
            Command::Interval(v) => self.snapshot.interval == v,
            Command::Threshold(v) => self.snapshot.threshold == v,
            Command::Notices => self.snapshot.notices,
            Command::Advanced(value) => self.advanced == value,
            Command::AudioPage(value) => self.audio_page == value,
            Command::AddDevice => true,
            _ => false,
        }
    }
    unsafe fn render(&self, hwnd: HWND) {
        for b in &self.controls {
            let enabled = match b.command {
                Command::Setting(action) => {
                    self.can_adjust()
                        && if let Page::Device(m) = self.page {
                            let s = &self.snapshot.devices[m];
                            match action {
                                Action::Dpi(_) => s.dpi.is_some(),
                                Action::Rate(_) => s.rate.is_some(),
                                Action::Sleep(_) => s.sleep_seconds.is_some(),
                                Action::Light(_) => s.light.is_some(),
                                Action::Stage(_) => s.active_stage.is_some(),
                                Action::Tune(key, _) => s.tuning.contains_key(&key),
                            }
                        } else {
                            false
                        }
                }
                Command::Step(_) => self.can_adjust(),
                Command::AudioStep(key, _) => {
                    self.can_adjust()
                        && if let Page::Device(m) = self.page {
                            self.tuning_value(m, key).is_some()
                        } else {
                            false
                        }
                }
                Command::FlatEq => {
                    self.can_adjust()
                        && if let Page::Device(m) = self.page {
                            crate::audio_fx::EQ_BANDS
                                .iter()
                                .all(|(key, _)| self.tuning_value(m, *key).is_some())
                        } else {
                            false
                        }
                }
                Command::ExportAudio | Command::ImportAudio => {
                    self.pending.is_none()
                        && self.queued.is_empty()
                        && if let Page::Device(m) = self.page {
                            self.snapshot.devices[m].fresh
                                && self.snapshot.devices[m].settings_error.is_empty()
                        } else {
                            false
                        }
                }
                Command::ConnectAudio
                | Command::DriverManager
                | Command::AddDevice
                | Command::Configuration
                | Command::CoreUpdate
                | Command::ControlsEditor
                | Command::ResidentMacros
                | Command::PresetLibrary
                | Command::HeadsetLighting
                | Command::GenericLighting => {
                    self.pending.is_none() && self.queued.is_empty() && !self.modal_open
                }
                Command::SettingsPage(delta) => {
                    if let Page::Device(m) = self.page {
                        if delta < 0 {
                            self.settings_page > 0
                        } else {
                            self.settings_page + 1 < self.settings_pages(m)
                        }
                    } else {
                        false
                    }
                }
                Command::HomePage(delta) => {
                    if delta < 0 {
                        self.home_page > 0
                    } else {
                        (self.home_page + 1) * if REGISTRY.len() > 3 { 4 } else { 3 }
                            < REGISTRY.len()
                    }
                }
                Command::Refresh => self.pending.is_none() && self.queued.is_empty(),
                _ => true,
            };
            EnableWindow(b.hwnd, enabled as i32);
            InvalidateRect(b.hwnd, null(), 0);
        }
        InvalidateRect(hwnd, null(), 0);
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dest = BeginPaint(hwnd, &mut ps);
        let mut r = zeroed();
        GetClientRect(hwnd, &mut r);
        let dc = CreateCompatibleDC(dest);
        let bitmap = CreateCompatibleBitmap(dest, r.right, r.bottom);
        let old = SelectObject(dc, bitmap);
        {
            let c = Canvas::new(dc, self.dpi);
            self.background(&c);
            match self.page {
                Page::Home => self.home(&c),
                Page::Device(m) => self.detail(&c, m),
                Page::Preferences => self.preferences(&c),
            }
            c.text(
                &self.status,
                if matches!(self.page,Page::Device(m) if REGISTRY[m].kind==Kind::Headset) {
                    R::new(562, self.height - 58, self.width - 880, 32)
                } else if matches!(self.page, Page::Device(_)) {
                    R::new(180, self.height - 58, self.width - 500, 32)
                } else if self.page == Page::Home {
                    R::new(
                        170,
                        self.height - 58,
                        if REGISTRY.len() > 4 {
                            (self.width / 2 - 300).max(120)
                        } else {
                            self.width - 340
                        },
                        32,
                    )
                } else {
                    R::new(28, self.height - 58, self.width - 350, 32)
                },
                12,
                400,
                if self.status_error { RED } else { DIM },
                DT_LEFT,
            );
        }
        BitBlt(dest, 0, 0, r.right, r.bottom, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
    unsafe fn background(&self, c: &Canvas) {
        c.rect(R::new(0, 0, self.width, self.height), BG);
        c.gradient(R::new(0, 0, self.width * 2 / 3, 68), 0xff261119, BG);
        c.line(28, 65, self.width - 28, 65, 0xff19191f, 1.);
        c.ellipse(
            R::new(if self.page == Page::Home { 28 } else { 186 }, 31, 7, 7),
            RED,
        );
        c.text(
            if self.page == Page::Home {
                "PERIPHERAL DESK  /  外设控制中心"
            } else {
                ""
            },
            R::new(
                if self.page == Page::Home { 47 } else { 206 },
                18,
                if self.page == Page::Home { 450 } else { 182 },
                32,
            ),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
    }
    unsafe fn home(&self, c: &Canvas) {
        c.text("我的设备", R::new(32, 83, 600, 45), 30, 600, TEXT, DT_LEFT);
        c.text(
            if REGISTRY.is_empty() {
                "尚未加载设备驱动，先从驱动仓库添加你的设备"
            } else {
                "选择设备，查看电量并调整设置"
            },
            R::new(34, 134, 700, 24),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        if REGISTRY.is_empty() {
            let center_y = 176 + (self.height - 266) / 2;
            c.round(
                R::new(32, 176, self.width - 64, self.height - 266),
                20,
                PANEL,
                Some(BORDER),
            );
            c.text(
                "从添加设备驱动开始",
                R::new(60, center_y - 98, self.width - 120, 42),
                28,
                600,
                TEXT,
                DT_CENTER,
            );
            c.text(
                "驱动按设备下载，只安装你需要的型号。",
                R::new(60, center_y - 44, self.width - 120, 26),
                15,
                400,
                MUTED,
                DT_CENTER,
            );
            c.text(
                "安装新设备驱动后，请退出并重新打开主程序。",
                R::new(60, center_y + 76, self.width - 120, 26),
                13,
                400,
                DIM,
                DT_CENTER,
            );
        }
    }
    unsafe fn detail(&self, c: &Canvas, m: usize) {
        let s = &self.snapshot.devices[m];
        let spec = &REGISTRY[m];
        let h = self.height;
        let rx = 394;
        let rw = self.width - rx - 28;
        let cx = rx + 30;
        c.round(R::new(28, 78, 344, h - 160), 20, PANEL, Some(BORDER));
        c.round(R::new(rx, 78, rw, h - 160), 20, PANEL, Some(BORDER));
        if s.fresh {
            c.glow(200, 185, 116);
        }
        c.image(self.images.images[m], R::new(60, 100, 280, 188), s.fresh);
        c.text(
            spec.name,
            R::new(54, 308, 300, 32),
            20,
            600,
            if s.fresh { TEXT } else { MUTED },
            DT_LEFT,
        );
        c.ellipse(R::new(56, 354, 7, 7), connection_color(s));
        c.text(
            connection(s, self.images.installed[m]),
            R::new(74, 340, 272, 32),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            spec.validation,
            R::new(54, 370, 292, 17),
            10,
            400,
            DIM,
            DT_LEFT,
        );
        c.line(54, 393, 346, 393, BORDER, 1.);
        if spec.kind == Kind::Accessory {
            c.text(
                "USB 供电 · 无电池",
                R::new(54, 415, 292, 30),
                18,
                500,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "灯效控制",
                R::new(424, 101, 400, 32),
                22,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "点击顶部“灯效设置”读取并编辑设备灯光。",
                R::new(424, 150, 660, 30),
                15,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "仅开放插件声明的模式、颜色与调节范围。",
                R::new(424, 194, 660, 30),
                13,
                400,
                DIM,
                DT_LEFT,
            );
            return;
        }
        if spec.kind == Kind::Headset {
            for (i, (label, key)) in [
                ("GAMING", Tuning::GameVolume),
                ("CHAT", Tuning::ChatVolume),
                ("麦克风", Tuning::MicVolume),
            ]
            .iter()
            .enumerate()
            {
                let y = 404 + i as i32 * 34;
                c.text(label, R::new(54, y, 150, 25), 13, 400, DIM, DT_LEFT);
                c.text(
                    &self
                        .tuning_value(m, *key)
                        .map(|v| format!("{v}%"))
                        .unwrap_or_else(|| "不可用".into()),
                    R::new(210, y, 136, 25),
                    13,
                    400,
                    MUTED,
                    DT_RIGHT,
                );
            }
            let y = (h - 312).max(520);
            c.round(R::new(112, y, 176, 112), 20, FIELD, Some(BORDER));
            c.text(
                "USB",
                R::new(112, y + 13, 176, 48),
                34,
                300,
                if s.fresh { TEXT } else { DIM },
                DT_CENTER,
            );
            c.text(
                "有线供电 · 无电池",
                R::new(90, y + 72, 220, 24),
                13,
                400,
                MUTED,
                DT_CENTER,
            );
            self.advanced_panel(c, m);
            return;
        }
        let pairs = [
            (
                "当前 DPI",
                s.dpi
                    .map(|v| format_number(v as u32))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "回报率",
                s.rate
                    .map(|v| format!("{v} Hz"))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "自动休眠",
                s.sleep_seconds
                    .map(|v| sleep_label(v, true))
                    .unwrap_or_else(|| "—".into()),
            ),
            ("供电状态", power(s).into()),
        ];
        for (i, (label, value)) in pairs.iter().enumerate() {
            if spec.kind == Kind::Keyboard && i == 0 {
                continue;
            }
            let y = 400 + i as i32 * 31;
            c.text(label, R::new(54, y, 120, 25), 13, 400, DIM, DT_LEFT);
            c.text(value, R::new(168, y, 178, 25), 13, 400, MUTED, DT_RIGHT);
        }
        let ring_y = (h - 303).max(538);
        c.ring(
            R::new(133, ring_y, 134, 134),
            s.percent.filter(|_| s.fresh),
            s.fresh,
        );
        c.text(
            if s.fresh {
                if s.charging == Some(true) {
                    "正在充电"
                } else if s.percent.is_some_and(|p| p < 20) {
                    "电量偏低"
                } else {
                    "电量充足"
                }
            } else {
                "等待设备连接"
            },
            R::new(60, ring_y + 150, 280, 24),
            13,
            400,
            MUTED,
            DT_CENTER,
        );
        if spec.read_only() {
            c.text(
                "只读设备状态",
                R::new(cx, 104, rw - 60, 34),
                23,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                spec.validation,
                R::new(cx, 152, rw - 60, 25),
                13,
                400,
                AMBER,
                DT_LEFT,
            );
            c.round(R::new(cx, 205, rw - 60, 134), 16, FIELD, Some(BORDER));
            c.text(
                "此插件提供设备识别与电量读取。",
                R::new(cx + 22, 225, rw - 104, 28),
                16,
                500,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "DPI、改键、宏和灯效写入未开放。",
                R::new(cx + 22, 268, rw - 104, 26),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                if s.fresh {
                    "当前读数已更新"
                } else {
                    &s.error
                },
                R::new(cx, 365, rw - 60, 70),
                13,
                400,
                if s.fresh { GREEN } else { DIM },
                DT_LEFT,
            );
            return;
        }
        if self.advanced && !spec.advanced.is_empty() {
            self.advanced_panel(c, m);
            return;
        }
        if spec.kind == Kind::Mouse {
            c.text(
                "DPI 灵敏度",
                R::new(cx, 101, 300, 30),
                20,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                &format!("点击快捷值或拖动滑块调整，滚轮可 {} 步进", spec.dpi_step),
                R::new(cx, 137, rw - 60, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            c.text(
                &self
                    .value(m)
                    .map(|v| format_number(v as u32))
                    .unwrap_or_else(|| "—".into()),
                R::new(rx + rw - 230, 97, 170, 52),
                38,
                300,
                if s.fresh { TEXT } else { DIM },
                DT_RIGHT,
            );
            c.text(
                "DPI",
                R::new(rx + rw - 53, 115, 30, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            let slider = self.slider;
            let y = slider.y + slider.h / 2;
            c.line(slider.x, y, slider.x + slider.w, y, BORDER, 7.);
            if let (Some(dpi), Some((min, max))) = (self.value(m), spec.dpi_range) {
                let pos = slider.x
                    + ((dpi.saturating_sub(min)) as f32 / (max - min) as f32 * slider.w as f32)
                        as i32;
                c.line(slider.x, y, pos, y, if s.fresh { RED } else { DIM }, 7.);
                c.ellipse(
                    R::new(pos - 15, y - 15, 30, 30),
                    if s.fresh { 0x25ff3859 } else { 0x255e5e6d },
                );
                c.ellipse(
                    R::new(pos - 9, y - 9, 18, 18),
                    if s.fresh { ORANGE } else { DIM },
                );
                c.ellipse(R::new(pos - 4, y - 4, 8, 8), TEXT);
            }
            c.text(
                "移动滑块可连续调整；点击 − / + 精调",
                R::new(cx, 286, rw - 60, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
        } else {
            c.text("键盘灯效", R::new(cx, 101, 300, 32), 20, 600, TEXT, DT_LEFT);
            c.text(
                "选择灯光效果，保留其他灯光及按键设置",
                R::new(cx, 137, rw - 60, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
        }
        c.line(cx, 337, rx + rw - 30, 337, BORDER, 1.);
        c.text("回报率", R::new(cx, 354, 160, 29), 15, 500, MUTED, DT_LEFT);
        c.text(
            "决定输入上报频率",
            R::new(cx, 382, 165, 22),
            11,
            400,
            DIM,
            DT_LEFT,
        );
        c.text(
            "自动休眠",
            R::new(cx, 421, 165, 29),
            15,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            "空闲后进入省电状态",
            R::new(cx, 451, 165, 22),
            11,
            400,
            DIM,
            DT_LEFT,
        );
        c.line(cx, 492, rx + rw - 30, 492, BORDER, 1.);
        c.text(
            "更多设备设置",
            R::new(cx, 513, 200, 27),
            15,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            if spec.advanced.is_empty() {
                "更多原驱动功能将逐步接入"
            } else {
                "点击上方「高级设置」，调整更多设备参数"
            },
            R::new(cx, 542, rw - 70, 24),
            13,
            400,
            DIM,
            DT_LEFT,
        );
        c.round(
            R::new(cx, h - 238, rw - 60, 64),
            12,
            0xff20151c,
            Some(0xff3c202b),
        );
        c.text(
            "自动保存",
            R::new(cx + 18, h - 230, rw - 96, 24),
            13,
            600,
            RED,
            DT_LEFT,
        );
        c.text(
            "停止调整 1.5 秒后写入设备，每次保存都会读回确认。",
            R::new(cx + 18, h - 204, rw - 96, 22),
            12,
            400,
            DIM,
            DT_LEFT,
        );
        c.round(R::new(cx, h - 160, rw - 60, 42), 10, FIELD, None);
        c.text(
            if !s.present {
                "设备未连接，设置暂不可用"
            } else if !s.fresh {
                "读数暂不可用，请唤醒设备并重新读取"
            } else if !s.settings_error.is_empty() {
                "电量已读取；部分设置暂不可用，请重新读取"
            } else {
                "已连接 · 设备设置可用"
            },
            R::new(cx + 16, h - 153, rw - 90, 27),
            12,
            400,
            if s.fresh { MUTED } else { DIM },
            DT_LEFT,
        );
    }
    unsafe fn advanced_panel(&self, c: &Canvas, m: usize) {
        let rx = 394;
        let rw = self.width - rx - 28;
        let cx = rx + 30;
        let h = self.height;
        let audio = REGISTRY[m].kind == Kind::Headset;
        c.text(
            if audio {
                match self.audio_page {
                    0 => "音量与麦克风",
                    1 => "十段均衡器",
                    2 => "空间与响度",
                    3 => "声音增强",
                    _ => "语音效果",
                }
            } else {
                "高级设置"
            },
            R::new(cx, 101, 350, 32),
            22,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            if audio {
                if self.audio_page == 1 {
                    "GAMING 通道 · ±20 dB · 每格 1 dB，拖动或按 ± 调整"
                } else if self.audio_page == 0 {
                    "Windows 音量同步 · − / + 可按 1% 微调"
                } else {
                    "保留原驱动音效 · 参数写入后读回确认"
                }
            } else {
                "从设备读取当前值，调整后自动保存并核对"
            },
            R::new(cx, 138, rw - 60, 23),
            13,
            400,
            DIM,
            DT_LEFT,
        );
        if !audio && self.settings_pages(m) > 1 {
            c.text(
                &format!("{} / {}", self.settings_page + 1, self.settings_pages(m)),
                R::new(rx + rw - 174, 101, 72, 34),
                12,
                400,
                MUTED,
                DT_CENTER,
            );
        }
        for (i, setting) in self.visible_settings(m).iter().enumerate() {
            let y = 169 + i as i32 * 78;
            c.text(
                setting.title,
                R::new(cx, y, 210, 27),
                15,
                500,
                TEXT,
                DT_LEFT,
            );
            c.text(
                &if is_volume(setting.key) {
                    format!(
                        "{} · 当前 {}",
                        setting.hint,
                        self.tuning_value(m, setting.key)
                            .map(|v| format!("{v}%"))
                            .unwrap_or_else(|| "不可用".into())
                    )
                } else {
                    setting.hint.into()
                },
                R::new(cx, y + 31, rw - 60, 22),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            c.line(cx, y + 65, rx + rw - 30, y + 65, BORDER, 1.);
        }
        if audio && self.audio_page == 1 {
            self.eq_panel(c, m);
        }
        c.round(R::new(cx, h - 182, rw - 60, 60), 12, FIELD, None);
        c.text(
            if audio {
                if self.snapshot.devices[m].settings_error.is_empty() {
                    "音效组件已连接 · 无需运行官方控制中心"
                } else {
                    "音效组件暂不可用"
                }
            } else {
                "自动保存 · 设备读回确认"
            },
            R::new(cx + 18, h - 172, rw - 96, 24),
            13,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            if audio {
                if self.snapshot.devices[m].settings_error.is_empty() {
                    "保留 HECATE / C-Media 驱动与音效组件"
                } else {
                    &self.snapshot.devices[m].settings_error
                }
            } else {
                "灰色选项表示当前设备或灯效未开放该设置"
            },
            R::new(cx + 18, h - 148, rw - 96, 20),
            12,
            400,
            DIM,
            DT_LEFT,
        );
    }
    unsafe fn preferences(&self, c: &Canvas) {
        c.text("工具设置", R::new(40, 97, 500, 45), 28, 600, TEXT, DT_LEFT);
        c.round(
            R::new(32, 170, self.width - 64, 430),
            20,
            PANEL,
            Some(BORDER),
        );
        for (y, title, hint) in [
            (209, "自动刷新", "较长间隔可减少设备唤醒"),
            (302, "低电阈值", "仅在未充电且读数有效时提醒"),
            (395, "低电提醒", "同一低电周期只提醒一次"),
            (487, "设备诊断", "仅导出本地 HID 元数据"),
        ] {
            c.text(title, R::new(62, y, 240, 28), 16, 500, TEXT, DT_LEFT);
            c.text(hint, R::new(62, y + 30, 250, 25), 12, 400, DIM, DT_LEFT);
        }
    }
    unsafe fn draw_button(&self, item: &DRAWITEMSTRUCT) {
        let dc = CreateCompatibleDC(item.hDC);
        let width = item.rcItem.right - item.rcItem.left;
        let height = item.rcItem.bottom - item.rcItem.top;
        let bitmap = CreateCompatibleBitmap(item.hDC, width, height);
        let old = SelectObject(dc, bitmap);
        let brush = CreateSolidBrush(0x000d0a0a);
        FillRect(dc, &item.rcItem, brush);
        DeleteObject(brush);
        let mut buffered = *item;
        buffered.hDC = dc;
        self.paint_button(&buffered);
        BitBlt(item.hDC, 0, 0, width, height, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
    }
    unsafe fn paint_button(&self, item: &DRAWITEMSTRUCT) {
        let Some(b) = self.controls.iter().find(|b| b.hwnd == item.hwndItem) else {
            return;
        };
        let brush = CreateSolidBrush(0x000d0a0a);
        FillRect(item.hDC, &item.rcItem, brush);
        DeleteObject(brush);
        let c = Canvas::new(item.hDC, self.dpi);
        let r = R::new(1, 1, b.rect.w - 2, b.rect.h - 2);
        let mut cursor: POINT = zeroed();
        GetCursorPos(&mut cursor);
        ScreenToClient(b.hwnd, &mut cursor);
        let hover = cursor.x >= 0
            && cursor.y >= 0
            && cursor.x < self.px(b.rect.w)
            && cursor.y < self.px(b.rect.h);
        if b.style == CARD {
            if let Command::Device(m) = b.command {
                self.card(&c, r, m);
                if hover || item.itemState & ODS_FOCUS != 0 {
                    c.round(
                        R::new(3, 3, b.rect.w - 6, b.rect.h - 6),
                        18,
                        0,
                        Some(if item.itemState & ODS_FOCUS != 0 {
                            RED
                        } else {
                            0xff66505c
                        }),
                    );
                }
            }
            return;
        }
        c.rect(
            R::new(0, 0, b.rect.w, b.rect.h),
            if b.style == CHIP || (b.rect.x >= 394 && b.rect.y >= 78 && b.rect.y < self.height - 82)
            {
                PANEL
            } else {
                BG
            },
        );
        if b.rect.y < 65 {
            c.gradient(
                R::new(-b.rect.x, -b.rect.y, self.width * 2 / 3, 68),
                0xff261119,
                BG,
            );
        }
        let disabled = item.itemState & ODS_DISABLED != 0;
        let selected = self.selected(b.command);
        let color = if disabled {
            0xff18181f
        } else if selected {
            RED
        } else if item.itemState & ODS_SELECTED != 0 {
            0xff33303a
        } else if hover {
            0xff292931
        } else {
            FIELD
        };
        if b.style != GHOST || selected || hover || item.itemState & ODS_SELECTED != 0 {
            c.round(r, 11, color, Some(if selected { RED } else { BORDER }));
        }
        c.text(
            &b.label,
            r,
            if b.label == "×" {
                25
            } else if b.command == Command::AddDevice {
                16
            } else {
                14
            },
            if selected { 500 } else { 400 },
            if disabled {
                DIM
            } else if selected {
                TEXT
            } else {
                MUTED
            },
            DT_CENTER,
        );
        if item.itemState & ODS_FOCUS != 0 {
            c.round(R::new(3, 3, b.rect.w - 6, b.rect.h - 6), 9, 0, Some(MUTED));
        }
    }
    unsafe fn card(&self, c: &Canvas, r: R, m: usize) {
        let s = &self.snapshot.devices[m];
        let spec = &REGISTRY[m];
        let active = s.fresh;
        c.rect(R::new(0, 0, r.w + 2, r.h + 2), BG);
        c.round(
            r,
            20,
            if active { PANEL } else { 0xff101014 },
            Some(if active { 0xff39313a } else { 0xff222229 }),
        );
        c.text(
            spec.brand,
            R::new(24, 20, r.w - 110, 22),
            12,
            500,
            if active { MUTED } else { DIM },
            DT_LEFT,
        );
        c.ellipse(R::new(r.w - 72, 27, 6, 6), connection_color(s));
        c.text(
            if active {
                "已连接"
            } else if !self.images.installed[m] {
                "未安装"
            } else if s.present {
                if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "未就绪"
                } else {
                    "待唤醒"
                }
            } else {
                "未连接"
            },
            R::new(r.w - 62, 18, 56, 24),
            11,
            400,
            connection_color(s),
            DT_LEFT,
        );
        if r.h < 400 {
            if active {
                c.glow(120, r.h / 2 + 12, 83);
            }
            let image_area = match spec.kind {
                Kind::Keyboard => R::new(18, 72, 196, r.h - 106),
                Kind::Mouse => R::new(36, 66, 160, r.h - 98),
                Kind::Headset | Kind::Accessory => R::new(26, 66, 180, r.h - 98),
            };
            c.image(self.images.images[m], image_area, active);
            c.text(
                spec.name,
                R::new(222, 72, r.w - 244, 34),
                22,
                600,
                if active { TEXT } else { DIM },
                DT_LEFT,
            );
            c.text(
                match spec.kind {
                    Kind::Mouse => "无线游戏鼠标",
                    Kind::Keyboard => "无线机械键盘",
                    Kind::Headset => "USB 双声卡耳机",
                    Kind::Accessory => "USB 外设",
                },
                R::new(224, 115, r.w - 248, 24),
                13,
                400,
                if active { MUTED } else { DIM },
                DT_LEFT,
            );
            c.text(
                spec.validation,
                R::new(224, 145, r.w - 248, 22),
                11,
                400,
                DIM,
                DT_LEFT,
            );
            c.line(224, r.h - 110, r.w - 24, r.h - 110, BORDER, 1.);
            c.text(
                if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "USB 有线供电"
                } else if !s.fresh && s.percent.is_some() {
                    "上次电量"
                } else {
                    "设备电量"
                },
                R::new(224, r.h - 99, 180, 22),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            c.text(
                &if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "USB".into()
                } else {
                    s.percent
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "—".into())
                },
                R::new(222, r.h - 72, 150, 43),
                32,
                300,
                if active { TEXT } else { DIM },
                DT_LEFT,
            );
            c.text(
                "设备设置  ›",
                R::new(r.w - 150, r.h - 65, 124, 32),
                13,
                500,
                if active { RED } else { DIM },
                DT_RIGHT,
            );
            return;
        }
        if active {
            c.glow(r.w / 2, 170, 110);
        }
        c.image(self.images.images[m], R::new(34, 60, r.w - 68, 222), active);
        c.text(
            spec.name,
            R::new(24, 309, r.w - 48, 34),
            20,
            600,
            if active { TEXT } else { DIM },
            DT_LEFT,
        );
        c.text(
            match spec.kind {
                Kind::Mouse => "无线游戏鼠标",
                Kind::Keyboard => "无线机械键盘",
                Kind::Headset => "USB 双声卡耳机",
                Kind::Accessory => "USB 外设",
            },
            R::new(24, 349, r.w - 48, 24),
            12,
            400,
            if active { MUTED } else { DIM },
            DT_LEFT,
        );
        c.line(24, r.h - 116, r.w - 24, r.h - 116, BORDER, 1.);
        c.text(
            if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                "供电方式"
            } else if !s.fresh && s.percent.is_some() {
                "上次电量"
            } else {
                "电量"
            },
            R::new(24, r.h - 102, 100, 25),
            12,
            400,
            DIM,
            DT_LEFT,
        );
        c.text(
            &if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                "USB".into()
            } else {
                s.percent
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "—".into())
            },
            R::new(24, r.h - 77, 140, 46),
            32,
            300,
            if active { TEXT } else { DIM },
            DT_LEFT,
        );
        c.text(
            if active {
                "设备设置  ›"
            } else {
                "查看设备  ›"
            },
            R::new(r.w - 145, r.h - 72, 120, 36),
            13,
            500,
            if active { RED } else { DIM },
            DT_RIGHT,
        );
    }
    unsafe fn tray(&mut self, hwnd: HWND) {
        let mut n = tray_data(hwnd);
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        n.uCallbackMessage = TRAY;
        n.hIcon = self.icon;
        let tip = self
            .snapshot
            .devices
            .iter()
            .zip(REGISTRY.iter())
            .map(|(s, spec)| {
                format!(
                    "{} {}",
                    spec.name,
                    if s.fresh && matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                        "USB 供电".into()
                    } else if s.fresh {
                        s.percent
                            .map(|p| format!("{p}%"))
                            .unwrap_or_else(|| "未知".into())
                    } else {
                        "未连接/待唤醒".into()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        copy_wide(
            &mut n.szTip,
            if REGISTRY.is_empty() {
                "Peripheral Desk · 到主页面添加设备驱动"
            } else {
                &tip
            },
        );
        self.tray_added =
            Shell_NotifyIconW(if self.tray_added { NIM_MODIFY } else { NIM_ADD }, &n) != 0;
    }
    unsafe fn notify(&self, hwnd: HWND, index: usize) {
        let mut n = tray_data(hwnd);
        n.uFlags = NIF_INFO;
        n.dwInfoFlags = NIIF_WARNING;
        copy_wide(
            &mut n.szInfoTitle,
            &format!("{} 电量偏低", REGISTRY[index].name),
        );
        copy_wide(
            &mut n.szInfo,
            &format!(
                "当前电量 {}%，请及时充电。",
                self.snapshot.devices[index].percent.unwrap()
            ),
        );
        Shell_NotifyIconW(NIM_MODIFY, &n);
    }
    unsafe fn menu(&mut self, hwnd: HWND) {
        let menu = CreatePopupMenu();
        for (id, label) in [
            (1, "打开主页面"),
            (2, "重新读取电量"),
            (4, "检查更新"),
            (3, "退出程序"),
        ] {
            AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
        }
        let mut p = zeroed();
        GetCursorPos(&mut p);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            p.x,
            p.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        match command {
            1 => {
                self.navigate(hwnd, Page::Home);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            2 => self.begin(hwnd, None),
            3 => self.quit(hwnd),
            4 => {
                ShowWindow(hwnd, SW_RESTORE);
                self.command(hwnd, Command::CoreUpdate);
            }
            _ => {}
        }
        PostMessageW(hwnd, WM_NULL, 0, 0);
    }
    unsafe fn show_battery(&mut self, hwnd: HWND) {
        if !self.flyout.is_null() && IsWindowVisible(self.flyout) != 0 {
            ShowWindow(self.flyout, SW_HIDE);
            return;
        }
        if self.flyout.is_null() {
            self.flyout = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                wide("PeripheralDeskBattery").as_ptr(),
                wide("Peripheral Desk · 设备电量").as_ptr(),
                WS_POPUP | WS_BORDER,
                0,
                0,
                1,
                1,
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                hwnd as _,
            );
        }
        let mut icon_rect = zeroed();
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: 1,
            guidItem: zeroed(),
        };
        let mut point = zeroed();
        GetCursorPos(&mut point);
        if Shell_NotifyIconGetRect(&identifier, &mut icon_rect) >= 0 {
            point.x = icon_rect.right;
            point.y = icon_rect.top;
        }
        let mut area: RECT = zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut area as *mut _ as _, 0);
        let width = self.px(350);
        let height = self.px(112 + REGISTRY.len().max(1) as i32 * 66);
        let x = (point.x - width).clamp(area.left, area.right - width);
        let y = (point.y - height - 12).clamp(area.top, area.bottom - height);
        SetWindowPos(
            self.flyout,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_SHOWWINDOW,
        );
        SetForegroundWindow(self.flyout);
        InvalidateRect(self.flyout, null(), 0);
    }
}

unsafe extern "system" fn flyout_proc(hwnd: HWND, msg: u32, _w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        return 1;
    }
    let main = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HWND;
    let ptr = if main.is_null() {
        null_mut()
    } else {
        GetWindowLongPtrW(main, GWLP_USERDATA) as *mut App
    };
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, _w, l);
    }
    let a = &*ptr;
    match msg {
        WM_KILLFOCUS => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            {
                let c = Canvas::new(dc, a.dpi);
                let height = 112 + REGISTRY.len().max(1) as i32 * 66;
                c.rect(R::new(0, 0, 350, height), PANEL);
                c.text("设备电量", R::new(21, 16, 240, 32), 17, 600, TEXT, DT_LEFT);
                c.text(
                    "PERIPHERAL DESK",
                    R::new(21, 44, 280, 20),
                    10,
                    400,
                    DIM,
                    DT_LEFT,
                );
                if REGISTRY.is_empty() {
                    c.text(
                        "未安装或尚未加载设备驱动",
                        R::new(21, 77, 308, 25),
                        14,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "到主页面添加 · 安装后重启主程序",
                        R::new(21, 105, 308, 22),
                        11,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                }
                for (i, spec) in REGISTRY.iter().enumerate() {
                    let s = &a.snapshot.devices[i];
                    let y = 76 + i as i32 * 66;
                    c.image(a.images.images[i], R::new(18, y, 42, 45), s.fresh);
                    c.text(
                        spec.name,
                        R::new(72, y, 158, 24),
                        13,
                        500,
                        if s.fresh { TEXT } else { DIM },
                        DT_LEFT,
                    );
                    c.text(
                        connection(s, a.images.installed[i]),
                        R::new(72, y + 25, 158, 20),
                        11,
                        400,
                        DIM,
                        DT_LEFT,
                    );
                    c.text(
                        &if s.fresh && matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                            "USB".into()
                        } else {
                            s.percent
                                .filter(|_| s.fresh)
                                .map(|p| format!("{p}%"))
                                .unwrap_or_else(|| "—".into())
                        },
                        R::new(236, y + 1, 93, 40),
                        25,
                        300,
                        if s.fresh { TEXT } else { DIM },
                        DT_RIGHT,
                    );
                }
                c.line(18, height - 40, 332, height - 40, BORDER, 1.);
                c.text(
                    "打开主页面  ›",
                    R::new(20, height - 34, 310, 28),
                    12,
                    500,
                    RED,
                    DT_RIGHT,
                );
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONUP => {
            let y = a.logical((l >> 16) as i16 as i32);
            let row = (y - 76) / 66;
            let device = if y >= 76 && row >= 0 && (row as usize) < REGISTRY.len() {
                row as usize + 1
            } else {
                0
            };
            ShowWindow(hwnd, SW_HIDE);
            PostMessageW(main, OPEN, device, 0);
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        _ => DefWindowProcW(hwnd, msg, _w, l),
    }
}
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_MOUSEMOVE {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut tracking);
        InvalidateRect(hwnd, null(), 0);
    }
    if msg == WM_MOUSELEAVE {
        InvalidateRect(hwnd, null(), 0);
    }
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(button_proc), 1);
    }
    DefSubclassProc(hwnd, msg, w, l)
}
fn coalesce(queue: &mut std::collections::VecDeque<(usize, Action)>, model: usize, action: Action) {
    queue.retain(|(id, old)| *id != model || !same_kind(*old, action));
    queue.push_back((model, action));
}
fn same_kind(a: Action, b: Action) -> bool {
    a.same_setting(b)
}
fn sleep_label(seconds: u16, full: bool) -> String {
    if seconds == 0 {
        "不休眠".into()
    } else if full {
        format!("{} 分钟", seconds / 60)
    } else {
        format!("{} 分", seconds / 60)
    }
}
fn is_volume(key: Tuning) -> bool {
    matches!(
        key,
        Tuning::GameVolume | Tuning::ChatVolume | Tuning::MicVolume
    )
}
fn format_number(v: u32) -> String {
    if v >= 1000 {
        format!("{},{:03}", v / 1000, v % 1000)
    } else {
        v.to_string()
    }
}
fn connection(s: &backend::State, installed: bool) -> &'static str {
    if !installed {
        return "请从驱动仓库安装插件";
    }
    if s.fresh {
        if s.wired {
            "USB 有线连接"
        } else {
            "2.4 GHz 无线连接"
        }
    } else if s.present {
        if REGISTRY
            .iter()
            .any(|spec| spec.key == s.key && matches!(spec.kind, Kind::Headset | Kind::Accessory))
        {
            "USB 已连接 · 状态未就绪"
        } else {
            "接收器在线 · 待唤醒"
        }
    } else {
        "设备未连接"
    }
}
fn connection_color(s: &backend::State) -> u32 {
    if s.fresh {
        GREEN
    } else if s.present {
        AMBER
    } else {
        DIM
    }
}
fn power(s: &backend::State) -> &'static str {
    if !s.fresh {
        "未知"
    } else {
        match s.charging {
            Some(true) => "正在充电",
            Some(false) => "电池供电",
            None => "状态未知",
        }
    }
}
fn copy_wide<const N: usize>(dest: &mut [u16; N], text: &str) {
    for (d, c) in dest[..N - 1].iter_mut().zip(text.encode_utf16()) {
        *d = c;
    }
}
unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..zeroed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_update_drain_coalesces_ready_devices_without_waiting() {
        let (tx, rx) = mpsc::channel();
        tx.send(WorkUpdate::Device(0, backend::State::default()))
            .unwrap();
        tx.send(WorkUpdate::Device(1, backend::State::default()))
            .unwrap();
        let (updates, disconnected) = drain_work_updates(&rx);
        assert!(!disconnected);
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], WorkUpdate::Device(0, _)));
        assert!(matches!(updates[1], WorkUpdate::Device(1, _)));
        let (updates, disconnected) = drain_work_updates(&rx);
        assert!(updates.is_empty());
        assert!(!disconnected);
    }
    #[test]
    fn work_update_drain_keeps_finished_after_device_updates() {
        let (tx, rx) = mpsc::channel();
        tx.send(WorkUpdate::Device(0, backend::State::default()))
            .unwrap();
        tx.send(WorkUpdate::Finished((Snapshot::default(), Ok(()))))
            .unwrap();
        let (updates, disconnected) = drain_work_updates(&rx);
        assert!(!disconnected);
        assert!(matches!(updates[0], WorkUpdate::Device(0, _)));
        assert!(matches!(updates[1], WorkUpdate::Finished(_)));
    }
    #[test]
    fn work_update_drain_keeps_ready_devices_on_disconnect() {
        let (tx, rx) = mpsc::channel();
        tx.send(WorkUpdate::Device(0, backend::State::default()))
            .unwrap();
        drop(tx);
        let (updates, disconnected) = drain_work_updates(&rx);
        assert!(disconnected);
        assert_eq!(updates.len(), 1);
        assert!(matches!(updates[0], WorkUpdate::Device(0, _)));
    }
    #[test]
    fn work_update_drain_returns_when_no_result_is_ready() {
        let (_tx, rx) = mpsc::channel();
        let (updates, disconnected) = drain_work_updates(&rx);
        assert!(updates.is_empty());
        assert!(!disconnected);
    }
    #[test]
    fn pending_changes_keep_other_devices_and_settings() {
        let mut q = std::collections::VecDeque::new();
        coalesce(&mut q, 0, Action::Dpi(800));
        coalesce(&mut q, 0, Action::Rate(500));
        coalesce(&mut q, 1, Action::Dpi(1600));
        coalesce(&mut q, 0, Action::Dpi(1200));
        assert_eq!(
            q.into_iter().collect::<Vec<_>>(),
            vec![
                (0, Action::Rate(500)),
                (1, Action::Dpi(1600)),
                (0, Action::Dpi(1200))
            ]
        );
    }
    #[test]
    fn tuning_queue_keeps_distinct_fields() {
        use crate::model::Tuning;
        let mut q = std::collections::VecDeque::new();
        coalesce(&mut q, 1, Action::Tune(Tuning::Lod, 1));
        coalesce(&mut q, 1, Action::Tune(Tuning::Motion, 0));
        coalesce(&mut q, 1, Action::Tune(Tuning::Lod, 2));
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].1, Action::Tune(Tuning::Motion, 0));
        assert_eq!(q[1].1, Action::Tune(Tuning::Lod, 2));
    }
    #[test]
    fn registry_and_view_model() {
        assert_eq!(format_number(30000), "30,000");
        assert_eq!(format_number(800), "800");
        let s = Snapshot::default();
        assert_eq!(s.devices.len(), REGISTRY.len());
        let mut keys = std::collections::HashSet::new();
        for spec in REGISTRY.iter() {
            assert!(keys.insert(spec.key));
            assert!(
                spec.image_bytes.is_empty(),
                "Device artwork must be delivered by its plugin"
            );
        }
    }
}
