use crate::{
    backend::{self, Action, Snapshot},
    devices::{Kind, LIGHTS, REGISTRY},
    hid::wide,
    model::Tuning,
    paint::{self, Canvas, Images, R, *},
    startup,
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

pub(crate) mod fx;
mod pages;
mod tray;
pub(crate) mod widgets;

use pages::{detail_y, is_volume, sleep_label};

pub(crate) use tray::pick_audio_component;
use tray::{flyout_proc, tray_data};

const DONE: u32 = WM_APP + 1;
const TRAY: u32 = WM_APP + 2;
const OPEN: u32 = WM_APP + 3;
const BATTERY: u32 = WM_APP + 4;
const CHIP: i32 = 1;
const GHOST: i32 = 3;
const CARD: i32 = 4;
const ENTRY: i32 = 5;
const SIDEBAR: i32 = 204;
const HOME_CONTENT_Y: i32 = 164;
fn widget_key(command: Command) -> u64 {
    match command {
        Command::Home => 0x100,
        Command::DriverManager => 0x101,
        Command::CoreUpdate => 0x102,
        Command::Prefs => 0x103,
        Command::Device(model) => 0x1000 + model as u64,
        Command::System(model) => 0x8000 + model as u64,
        _ => 0,
    }
}
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
    System(usize),
    Preferences,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Home,
    Device(usize),
    System(usize),
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
    Startup,
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
    startup_enabled: bool,
    images: Images,
    icon: HICON,
    tray_added: bool,
    taskbar_created: u32,
    flyout: HWND,
    main: HWND,
    pub(crate) fx: fx::Fx,
}

pub fn message(text: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("外设盒").as_ptr(),
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
            startup_enabled: startup::is_enabled().unwrap_or(false),
            images,
            icon,
            tray_added: false,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            flyout: null_mut(),
            main: null_mut(),
            fx: fx::Fx::default(),
        });
        let dpi = app.dpi;
        let raw = &mut *app as *mut App;
        let hwnd = CreateWindowExW(
            0,
            wide("PeripheralDeskWindow").as_ptr(),
            wide("外设盒 · 外设控制中心").as_ptr(),
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
        WM_TIMER if w == 3 => {
            if a.tick_fx(hwnd) {
                InvalidateRect(hwnd, null(), 0);
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
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tracking);
            let (hx, hy) = a.mouse(l);
            a.on_hover_move(hx, hy);
            let mut changed = false;
            for b in &a.controls {
                let key = widget_key(b.command);
                if key != 0 && a.fx.widget.set_hover(key, b.rect.contains(hx, hy)) {
                    if b.rect.contains(hx, hy) {
                        a.fx.widget.animate(key, 0., 0);
                        a.fx.widget.animate(key, 1., 130);
                    } else {
                        a.fx.widget.animate(key, 0., 130);
                    }
                    changed = true;
                }
            }
            if changed {
                a.fx_wake(hwnd);
                InvalidateRect(hwnd, null(), 0);
            }
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
        WM_MOUSELEAVE => {
            a.on_hover_leave();
            InvalidateRect(hwnd, null(), 0);
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
        WM_QUERYENDSESSION => i32::from(
            !a.modal_open
                && a.inflight.is_none()
                && a.profile_notice.is_none()
                && a.queued.is_empty(),
        ) as isize,
        WM_ENDSESSION => {
            if w != 0 {
                DestroyWindow(hwnd);
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
    fn update_ring_targets(&mut self) {
        for (i, state) in self.snapshot.devices.iter().enumerate() {
            self.fx.widget.ring_target(0x1000 + i as u64, state.percent);
        }
        for (i, device) in self.snapshot.system_devices.iter().enumerate() {
            self.fx
                .widget
                .ring_target(0x8000 + i as u64, device.state.percent);
        }
    }
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
        self.fx.widget.clear_press();
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
        let entries = [
            ("我的设备", Command::Home),
            ("驱动商店", Command::DriverManager),
            ("检查更新", Command::CoreUpdate),
            ("偏好设置", Command::Prefs),
        ];
        for (i, (label, command)) in entries.into_iter().enumerate() {
            self.add(
                hwnd,
                label,
                R::new(12, 66 + i as i32 * 44, SIDEBAR - 24, 38),
                ENTRY,
                command,
            );
        }
        let total_devices = REGISTRY.len() + self.snapshot.system_devices.len();
        let page_size = 4;
        let pages = total_devices.div_ceil(page_size).max(1);
        self.home_page = self.home_page.min(pages - 1);
        for (slot, index) in (self.home_page * page_size..total_devices)
            .take(page_size)
            .enumerate()
        {
            let name = if index < REGISTRY.len() {
                REGISTRY[index].name.to_owned()
            } else {
                self.snapshot.system_devices[index - REGISTRY.len()]
                    .name
                    .clone()
            };
            self.add(
                hwnd,
                &name,
                R::new(12, 286 + slot as i32 * 42, SIDEBAR - 24, 38),
                ENTRY,
                if index < REGISTRY.len() {
                    Command::Device(index)
                } else {
                    Command::System(index - REGISTRY.len())
                },
            );
        }
        if pages > 1 {
            self.add(
                hwnd,
                "‹",
                R::new(34, h - 58, 44, 34),
                GHOST,
                Command::HomePage(-1),
            );
            self.add(
                hwnd,
                "›",
                R::new(126, h - 58, 44, 34),
                GHOST,
                Command::HomePage(1),
            );
        }
        match self.page {
            Page::Home => {
                let total = REGISTRY.len() + self.snapshot.system_devices.len();
                if total == 0 {
                    let center_y = HOME_CONTENT_Y + (h - HOME_CONTENT_Y - 86) / 2;
                    self.add(
                        hwnd,
                        "＋  添加设备驱动",
                        R::new(w / 2 - 132, center_y + 8, 264, 48),
                        CHIP,
                        Command::AddDevice,
                    );
                }
                let cols = if w - SIDEBAR > 1040 { 3 } else { 2 };
                let cw = ((w - SIDEBAR - 52 - 18 * (cols - 1)) / cols).max(250);
                let card_h = ((h - HOME_CONTENT_Y - 110) / 2).clamp(250, 360);
                for (slot, index) in (self.home_page * 4..total).take(4).enumerate() {
                    let x = SIDEBAR + 24 + (slot as i32 % cols) * (cw + 18);
                    let y = HOME_CONTENT_Y + slot as i32 / cols * (card_h + 16);
                    if y + card_h > h - 78 {
                        break;
                    }
                    let name = if index < REGISTRY.len() {
                        REGISTRY[index].name.to_owned()
                    } else {
                        self.snapshot.system_devices[index - REGISTRY.len()]
                            .name
                            .clone()
                    };
                    self.add(
                        hwnd,
                        &name,
                        R::new(x, y, cw, card_h),
                        CARD,
                        if index < REGISTRY.len() {
                            Command::Device(index)
                        } else {
                            Command::System(index - REGISTRY.len())
                        },
                    );
                }
                self.add(
                    hwnd,
                    "重新读取",
                    R::new(w - 268, h - 56, 116, 34),
                    GHOST,
                    Command::Refresh,
                );
                self.add(
                    hwnd,
                    "配置备份",
                    R::new(w - 396, h - 56, 116, 34),
                    GHOST,
                    Command::Configuration,
                );
                self.add(
                    hwnd,
                    "退出程序",
                    R::new(w - 144, h - 56, 112, 34),
                    GHOST,
                    Command::Quit,
                );
            }
            Page::Device(m) => {
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
                        "导入音效组件",
                        R::new(SIDEBAR + 16, h - 62, 148, 38),
                        GHOST,
                        Command::ConnectAudio,
                    );
                    self.add(
                        hwnd,
                        "导出音效",
                        R::new(SIDEBAR + 176, h - 62, 96, 38),
                        GHOST,
                        Command::ExportAudio,
                    );
                    self.add(
                        hwnd,
                        "导入音效",
                        R::new(SIDEBAR + 284, h - 62, 96, 38),
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
                            R::new(rx + rw - 240, detail_y(101), 64, 34),
                            GHOST,
                            Command::SettingsPage(-1),
                        );
                        self.add(
                            hwnd,
                            "下一页",
                            R::new(rx + rw - 94, detail_y(101), 64, 34),
                            GHOST,
                            Command::SettingsPage(1),
                        );
                    }
                    for (row, setting) in self.visible_settings(m).iter().enumerate() {
                        let y = detail_y(172 + row as i32 * 78);
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
                                R::new(x, detail_y(272), bw - 10, 30),
                                CHIP,
                                Command::AudioStep(*key, 1),
                            );
                            self.add(
                                hwnd,
                                "−",
                                R::new(x, detail_y(522), bw - 10, 30),
                                CHIP,
                                Command::AudioStep(*key, -1),
                            );
                        }
                        self.add(
                            hwnd,
                            "所有频段归零",
                            R::new(cx, detail_y(586), 156, 36),
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
                            R::new(cx + i as i32 * (bw + 10), detail_y(178), bw, 38),
                            CHIP,
                            Command::Setting(action),
                        );
                    }
                    self.slider = R::new(cx, detail_y(239), rw - 158, 24);
                    self.add(
                        hwnd,
                        "−",
                        R::new(rx + rw - 112, detail_y(232), 34, 34),
                        GHOST,
                        Command::Step(-50),
                    );
                    self.add(
                        hwnd,
                        "+",
                        R::new(rx + rw - 68, detail_y(232), 34, 34),
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
                                detail_y(172 + (i / 5) as i32 * 48),
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
                            R::new(x + i as i32 * (bw + gap), detail_y(358), bw, 37),
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
                            R::new(x, detail_y(425), width, 35),
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
            Page::System(_) => {
                self.add(
                    hwnd,
                    "重新读取",
                    R::new(w - 150, h - 61, 122, 39),
                    GHOST,
                    Command::Refresh,
                );
            }
            Page::Preferences => {
                for (i, value) in [30, 60, 120, 300, 600, 1800].iter().enumerate() {
                    self.add(
                        hwnd,
                        &format!("{value} 秒"),
                        R::new(330 + i as i32 * 116, 251, 104, 36),
                        CHIP,
                        Command::Interval(*value),
                    );
                }
                for (i, value) in [5, 10, 15, 20, 25, 30, 40, 50].iter().enumerate() {
                    self.add(
                        hwnd,
                        &format!("{value}%"),
                        R::new(330 + i as i32 * 84, 346, 72, 36),
                        CHIP,
                        Command::Threshold(*value),
                    );
                }
                self.add(
                    hwnd,
                    "启用低电提醒",
                    R::new(330, 441, 180, 40),
                    CHIP,
                    Command::Notices,
                );
                self.add(
                    hwnd,
                    "导出设备诊断",
                    R::new(330, 536, 180, 40),
                    GHOST,
                    Command::Diagnose,
                );
                self.add(
                    hwnd,
                    if self.startup_enabled {
                        "开机启动：已开启"
                    } else {
                        "开机启动：关闭"
                    },
                    R::new(330, 625, 200, 40),
                    CHIP,
                    Command::Startup,
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
        match page {
            Page::Device(model) => self.home_page = model / 4,
            Page::System(model) => self.home_page = (REGISTRY.len() + model) / 4,
            Page::Home | Page::Preferences => {}
        }
        self.page = page;
        self.advanced = false;
        self.settings_page = 0;
        self.audio_page = 0;
        self.preview_dpi = None;
        self.fx.nav.start();
        self.fx_wake(hwnd);
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
                let restarting = crate::plugin_ui::open(hwnd);
                self.modal_open = false;
                if restarting {
                    DestroyWindow(hwnd);
                    return;
                }
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
                let total = REGISTRY.len() + self.snapshot.system_devices.len();
                self.home_page = (self.home_page as i32 + delta).clamp(
                    0,
                    (total.div_ceil(if total > 3 { 4 } else { 3 }) - 1) as i32,
                ) as usize;
                self.layout(hwnd);
            }
            Command::Home => self.navigate(hwnd, Page::Home),
            Command::Device(m) => self.navigate(hwnd, Page::Device(m)),
            Command::System(m) => self.navigate(hwnd, Page::System(m)),
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
            Command::Startup => match startup::set_enabled(!self.startup_enabled) {
                Ok(()) => {
                    self.startup_enabled = !self.startup_enabled;
                    self.set_status(
                        if self.startup_enabled {
                            "已开启开机启动，登录 Windows 后将在托盘运行"
                        } else {
                            "已关闭开机启动"
                        },
                        false,
                    );
                    self.layout(hwnd);
                }
                Err(error) => self.set_status(&error, true),
            },
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
            } else if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
                "打开驱动仓库，添加所需设备驱动；安装后自动重启生效"
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
        let selected_system = match self.page {
            Page::System(index) => self
                .snapshot
                .system_devices
                .get(index)
                .map(|device| device.state.key.clone()),
            _ => None,
        };
        self.snapshot = s;
        if let Some(key) = selected_system {
            self.page = self
                .snapshot
                .system_devices
                .iter()
                .position(|device| device.state.key == key)
                .map(Page::System)
                .unwrap_or(Page::Home);
        }
        self.update_ring_targets();
        self.fx_wake(hwnd);
        let total = REGISTRY.len() + self.snapshot.system_devices.len();
        let page_size = 4;
        self.home_page = self.home_page.min(total.saturating_sub(1) / page_size);
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
                let count = self.snapshot.devices.iter().filter(|d| d.fresh).count()
                    + self.snapshot.system_devices.len();
                self.set_status(
                    &if let Some(notice) = profile_notice {
                        notice.into()
                    } else if writing {
                        "设置已保存，读回确认成功".into()
                    } else if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
                        "打开驱动仓库添加设备；新驱动安装后自动重启生效".into()
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
    unsafe fn connect_audio(&mut self, hwnd: HWND) {
        if self.pending.is_some() || !self.queued.is_empty() || self.modal_open {
            return;
        }
        self.modal_open = true;
        let result = pick_audio_component(hwnd);
        self.modal_open = false;
        match result {
            Ok(Some(path)) => match crate::audio_fx::configure(&path) {
                Ok(()) => {
                    self.set_status("音效组件已导入设备目录，正在读取…", false);
                    self.begin(hwnd, None);
                }
                Err(error) => self.set_status(&error, true),
            },
            Ok(None) => {}
            Err(error) => self.set_status(&error, true),
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
            Command::Startup => self.startup_enabled,
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
                    let total = REGISTRY.len() + self.snapshot.system_devices.len();
                    let page_size = 4;
                    if delta < 0 {
                        self.home_page > 0
                    } else {
                        (self.home_page + 1) * page_size < total
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
                Page::Device(m) => {
                    self.detail(&c, m);
                    self.device_summary(&c, m);
                }
                Page::System(m) => self.system_detail(&c, m),
                Page::Preferences => self.preferences(&c),
            }
            self.sidebar_overlay(&c);
            c.text(
                &self.status,
                if matches!(self.page,Page::Device(m) if REGISTRY[m].kind==Kind::Headset) {
                    R::new(562, self.height - 58, self.width - 880, 32)
                } else if matches!(self.page, Page::Device(_)) {
                    R::new(
                        SIDEBAR + 24,
                        self.height - 54,
                        self.width - SIDEBAR - 320,
                        30,
                    )
                } else if self.page == Page::Home {
                    R::new(
                        SIDEBAR + 24,
                        self.height - 52,
                        self.width - SIDEBAR - 420,
                        28,
                    )
                } else {
                    R::new(
                        SIDEBAR + 24,
                        self.height - 54,
                        self.width - SIDEBAR - 350,
                        30,
                    )
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
        let parent = GetParent(hwnd);
        let mut point = POINT {
            x: l as i16 as i32,
            y: (l >> 16) as i16 as i32,
        };
        MapWindowPoints(hwnd, parent, &mut point, 1);
        let packed = ((point.y as u16 as isize) << 16) | point.x as u16 as isize;
        SendMessageW(parent, WM_MOUSEMOVE, w, packed);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::pages::format_number;

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
