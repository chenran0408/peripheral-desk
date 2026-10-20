use crate::{
    backend::{self, Action, Snapshot, NAMES},
    hid::wide,
};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{
        Controls::*, HiDpi::*, Input::KeyboardAndMouse::EnableWindow, Shell::*,
        WindowsAndMessaging::*,
    },
};

const DONE: u32 = WM_APP + 1;
const TRAY: u32 = WM_APP + 2;
const REFRESH: usize = 100;
const DEVICE: usize = 101;
const DPI_EDIT: usize = 102;
const DPI_APPLY: usize = 103;
const RATE: usize = 104;
const RATE_APPLY: usize = 105;
const SLEEP: usize = 106;
const SLEEP_APPLY: usize = 107;
const LIGHT: usize = 108;
const LIGHT_APPLY: usize = 109;
const INTERVAL: usize = 110;
const THRESHOLD: usize = 111;
const NOTICES: usize = 112;
const PREFS: usize = 113;
const DIAGNOSE: usize = 114;
const EXIT: usize = 115;
const INK: u32 = 0x00352a21;
const MUTED: u32 = 0x0087796a;
const BG: u32 = 0x00faf7f3;
const ACCENT: u32 = 0x00b87d21;
const GREEN: u32 = 0x006c8a18;
const BORDER: u32 = 0x00e8e2dc;
const SS_LEFT: u32 = 0;

type WorkResult = (Snapshot, std::result::Result<(), String>);
struct App {
    snapshot: Snapshot,
    pending: Option<Receiver<WorkResult>>,
    controls: Vec<(usize, HWND)>,
    cards: Vec<HWND>,
    detail: HWND,
    status: HWND,
    font: HFONT,
    dpi: u32,
    tray_icons: [HICON; 3],
    tray_added: [bool; 3],
    taskbar_created: u32,
    mutex: HANDLE,
    rates: Vec<u16>,
    sleeps: Vec<u16>,
    lights: Vec<u8>,
    writing: bool,
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

pub fn run(hidden: bool) {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(null(), 0, wide("Local\\PeripheralDesk-0.1").as_ptr());
        if mutex.is_null() {
            message("无法创建程序实例锁");
            return;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let existing = FindWindowW(wide("PeripheralDeskWindow").as_ptr(), null());
            if !existing.is_null() {
                ShowWindow(existing, SW_RESTORE);
                SetForegroundWindow(existing);
            }
            CloseHandle(mutex);
            return;
        }
        let instance = GetModuleHandleW(null());
        let class = wide("PeripheralDeskWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            ..zeroed()
        };
        RegisterClassExW(&wc);
        let mut app = Box::new(App {
            snapshot: Snapshot::load(),
            pending: None,
            controls: vec![],
            cards: vec![],
            detail: null_mut(),
            status: null_mut(),
            font: null_mut(),
            dpi: 96,
            tray_icons: [null_mut(); 3],
            tray_added: [false; 3],
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            mutex,
            rates: vec![],
            sleeps: vec![],
            lights: vec![0, 1, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13, 15, 16, 17],
            writing: false,
        });
        let raw = &mut *app as *mut App;
        let dpi = GetDpiForSystem();
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Peripheral Desk · 外设控制台").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (1110 * dpi / 96) as i32,
            (790 * dpi / 96) as i32,
            null_mut(),
            null_mut(),
            instance,
            raw as _,
        );
        if hwnd.is_null() {
            CloseHandle(app.mutex);
            message("创建窗口失败");
            return;
        }
        if !hidden {
            ShowWindow(hwnd, SW_SHOW);
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let app = &mut *ptr;
    if msg == app.taskbar_created {
        app.tray_added = [false; 3];
        app.trays(hwnd);
        return 0;
    }
    match msg {
        WM_CREATE => {
            app.dpi = GetDpiForWindow(hwnd);
            app.init(hwnd);
            app.layout(hwnd);
            app.populate();
            app.render();
            SetTimer(hwnd, 1, app.snapshot.interval * 1000, None);
            app.begin(hwnd, None);
            0
        }
        WM_SIZE => {
            app.layout(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(l as *mut MINMAXINFO);
            m.ptMinTrackSize.x = app.px(1040);
            m.ptMinTrackSize.y = app.px(760);
            0
        }
        WM_DPICHANGED => {
            app.dpi = (w & 0xffff) as u32;
            let r = &*(l as *const RECT);
            app.reset_font();
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            0
        }
        WM_PAINT => {
            app.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLORSTATIC => {
            let dc = w as HDC;
            SetTextColor(dc, INK);
            SetBkMode(dc, TRANSPARENT as i32);
            SetDCBrushColor(
                dc,
                if app.cards.contains(&(l as HWND)) || l as HWND == app.detail {
                    0xffffff
                } else {
                    BG
                },
            );
            GetStockObject(DC_BRUSH) as isize
        }
        WM_DRAWITEM => {
            app.draw_button(&*(l as *const DRAWITEMSTRUCT));
            1
        }
        WM_TIMER if w == 1 => {
            app.begin(hwnd, None);
            0
        }
        DONE => {
            app.complete(hwnd);
            0
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            match id {
                REFRESH => app.begin(hwnd, None),
                DEVICE if (w >> 16) & 0xffff == CBN_SELCHANGE as usize => {
                    app.populate();
                    app.render();
                }
                DPI_APPLY | RATE_APPLY | SLEEP_APPLY | LIGHT_APPLY => app.apply(hwnd, id),
                PREFS => app.preferences(hwnd),
                DIAGNOSE => app.diagnose(),
                EXIT => {
                    app.quit(hwnd);
                }
                _ => {}
            }
            0
        }
        TRAY => {
            match l as u32 {
                WM_LBUTTONUP => {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
                WM_LBUTTONDBLCLK => app.begin(hwnd, None),
                WM_RBUTTONUP => {
                    let menu = CreatePopupMenu();
                    for (id, label) in [(201, "打开面板"), (202, "立即刷新"), (203, "退出程序")]
                    {
                        AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
                    }
                    let mut p = zeroed();
                    GetCursorPos(&mut p);
                    SetForegroundWindow(hwnd);
                    let cmd = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        p.x,
                        p.y,
                        0,
                        hwnd,
                        null(),
                    );
                    DestroyMenu(menu);
                    match cmd {
                        201 => {
                            ShowWindow(hwnd, SW_RESTORE);
                        }
                        202 => app.begin(hwnd, None),
                        203 => {
                            app.quit(hwnd);
                        }
                        _ => {}
                    }
                    PostMessageW(hwnd, WM_NULL, 0, 0);
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            for id in 0..3 {
                let mut n: NOTIFYICONDATAW = zeroed();
                n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
                n.hWnd = hwnd;
                n.uID = id as u32 + 1;
                Shell_NotifyIconW(NIM_DELETE, &n);
                if !app.tray_icons[id].is_null() {
                    DestroyIcon(app.tray_icons[id]);
                }
            }
            if let Err(e) = app.snapshot.save() {
                message(&format!("保存偏好失败：{e}"));
            }
            DeleteObject(app.font);
            CloseHandle(app.mutex);
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
    unsafe fn quit(&self,hwnd:HWND) {
        if self.pending.is_some() && self.writing {
            self.set(self.status,"正在保存设备设置，请等待写入完成后退出");
        } else {DestroyWindow(hwnd);}
    }
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    fn get(&self, id: usize) -> HWND {
        self.controls.iter().find(|(key, _)| *key == id).unwrap().1
    }
    unsafe fn control(
        &mut self,
        parent: HWND,
        class: &str,
        text: &str,
        id: usize,
        style: u32,
    ) -> HWND {
        let c = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            parent,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(c, WM_SETFONT, self.font as usize, 1);
        if id > 0 {
            self.controls.push((id, c));
        }
        c
    }
    unsafe fn reset_font(&mut self) {
        let previous = self.font;
        self.font = font(-self.px(14), 400);
        for (_, c) in &self.controls {
            SendMessageW(*c, WM_SETFONT, self.font as usize, 1);
        }
        for c in &self.cards {
            SendMessageW(*c, WM_SETFONT, self.font as usize, 1);
        }
        for c in [self.detail, self.status] {
            if !c.is_null() {
                SendMessageW(c, WM_SETFONT, self.font as usize, 1);
            }
        }
        if !previous.is_null() {
            DeleteObject(previous);
        }
    }
    unsafe fn init(&mut self, hwnd: HWND) {
        self.reset_font();
        for _ in 0..3 {
            let c = self.control(hwnd, "STATIC", "正在读取…", 0, SS_LEFT);
            self.cards.push(c);
        }
        self.detail = self.control(hwnd, "STATIC", "", 0, SS_LEFT);
        self.status = self.control(hwnd, "STATIC", "准备就绪", 0, SS_LEFT);
        self.control(
            hwnd,
            "BUTTON",
            "立即刷新",
            REFRESH,
            BS_OWNERDRAW as u32 | WS_TABSTOP,
        );
        for id in [DEVICE, RATE, SLEEP, LIGHT, INTERVAL, THRESHOLD] {
            self.control(
                hwnd,
                "COMBOBOX",
                "",
                id,
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
            );
        }
        for n in NAMES {
            SendMessageW(self.get(DEVICE), CB_ADDSTRING, 0, wide(n).as_ptr() as isize);
        }
        SendMessageW(self.get(DEVICE), CB_SETCURSEL, 0, 0);
        self.control(
            hwnd,
            "EDIT",
            "",
            DPI_EDIT,
            WS_BORDER | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
        );
        for (id, title) in [
            (DPI_APPLY, "应用 DPI"),
            (RATE_APPLY, "应用回报率"),
            (SLEEP_APPLY, "应用休眠"),
            (LIGHT_APPLY, "应用灯效"),
            (PREFS, "保存偏好"),
            (DIAGNOSE, "导出诊断"),
            (EXIT, "退出程序"),
        ] {
            self.control(hwnd, "BUTTON", title, id, BS_OWNERDRAW as u32 | WS_TABSTOP);
        }
        self.control(
            hwnd,
            "BUTTON",
            "低电提醒",
            NOTICES,
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        );
        SendMessageW(
            self.get(NOTICES),
            BM_SETCHECK,
            if self.snapshot.notices {
                BST_CHECKED
            } else {
                BST_UNCHECKED
            } as usize,
            0,
        );
        for n in [30, 60, 120, 300, 600, 1800] {
            SendMessageW(
                self.get(INTERVAL),
                CB_ADDSTRING,
                0,
                wide(&format!("{n} 秒")).as_ptr() as isize,
            );
        }
        SendMessageW(
            self.get(INTERVAL),
            CB_SETCURSEL,
            [30, 60, 120, 300, 600, 1800]
                .iter()
                .position(|v| *v == self.snapshot.interval)
                .unwrap_or(1),
            0,
        );
        for n in [5, 10, 15, 20, 25, 30, 40, 50] {
            SendMessageW(
                self.get(THRESHOLD),
                CB_ADDSTRING,
                0,
                wide(&format!("{n} %")).as_ptr() as isize,
            );
        }
        SendMessageW(
            self.get(THRESHOLD),
            CB_SETCURSEL,
            [5, 10, 15, 20, 25, 30, 40, 50]
                .iter()
                .position(|v| *v == self.snapshot.threshold)
                .unwrap_or(3),
            0,
        );
    }
    unsafe fn place(&self, c: HWND, x: i32, y: i32, w: i32, h: i32) {
        MoveWindow(c, self.px(x), self.px(y), self.px(w), self.px(h), 1);
    }
    unsafe fn layout(&self, hwnd: HWND) {
        let mut r = zeroed();
        GetClientRect(hwnd, &mut r);
        let width = (r.right * 96 / self.dpi as i32).max(1000);
        let cw = (width - 80) / 3;
        self.place(self.get(REFRESH), width - 150, 28, 118, 38);
        for i in 0..3 {
            self.place(self.cards[i], 50 + i as i32 * (cw + 16), 186, cw - 36, 88);
        }
        self.place(self.get(DEVICE), 48, 352, 270, 180);
        self.place(self.detail, 344, 353, width - 395, 64);
        let col = (width - 128) / 4;
        for (i, (edit, apply)) in [
            (DPI_EDIT, DPI_APPLY),
            (RATE, RATE_APPLY),
            (SLEEP, SLEEP_APPLY),
            (LIGHT, LIGHT_APPLY),
        ]
        .iter()
        .enumerate()
        {
            let x = 48 + i as i32 * (col + 16);
            self.place(
                self.get(*edit),
                x,
                449,
                col,
                if *edit == DPI_EDIT { 32 } else { 220 },
            );
            self.place(self.get(*apply), x, 492, col, 34);
        }
        self.place(self.get(INTERVAL), 135, 610, 118, 160);
        self.place(self.get(THRESHOLD), 348, 610, 90, 220);
        self.place(self.get(NOTICES), 465, 612, 120, 30);
        self.place(self.get(PREFS), width - 420, 608, 112, 36);
        self.place(self.get(DIAGNOSE), width - 294, 608, 112, 36);
        self.place(self.get(EXIT), width - 168, 608, 120, 36);
        self.place(self.status, 34, 682, width - 68, 44);
    }
    unsafe fn set(&self, c: HWND, s: &str) {
        SetWindowTextW(c, wide(s).as_ptr());
    }
    unsafe fn selected(&self) -> usize {
        SendMessageW(self.get(DEVICE), CB_GETCURSEL, 0, 0).clamp(0, 2) as usize
    }
    unsafe fn populate(&mut self) {
        let m = self.selected();
        let d = &self.snapshot.devices[m];
        self.rates = if m == 0 {
            vec![125, 500, 1000]
        } else if m == 2 {
            vec![250, 500, 1000]
        } else if d.wired {
            vec![125, 500, 1000, 2000, 4000, 8000]
        } else {
            vec![125, 500, 1000, 2000, 4000]
        };
        self.sleeps = if m == 0 {
            vec![60, 120, 180, 300, 600, 900]
        } else if m == 2 {
            vec![60, 120, 300, 600, 900, 1200]
        } else {
            vec![60, 120, 300, 600, 900, 1800, 3600]
        };
        SendMessageW(self.get(RATE), CB_RESETCONTENT, 0, 0);
        for v in &self.rates {
            SendMessageW(
                self.get(RATE),
                CB_ADDSTRING,
                0,
                wide(&format!("{v} Hz")).as_ptr() as isize,
            );
        }
        SendMessageW(
            self.get(RATE),
            CB_SETCURSEL,
            self.rates
                .iter()
                .position(|v| Some(*v) == d.rate)
                .unwrap_or(2),
            0,
        );
        SendMessageW(self.get(SLEEP), CB_RESETCONTENT, 0, 0);
        for v in &self.sleeps {
            SendMessageW(
                self.get(SLEEP),
                CB_ADDSTRING,
                0,
                wide(&format!("{} 分钟", v / 60)).as_ptr() as isize,
            );
        }
        SendMessageW(
            self.get(SLEEP),
            CB_SETCURSEL,
            self.sleeps
                .iter()
                .position(|v| Some(*v) == d.sleep_seconds)
                .unwrap_or(0),
            0,
        );
        SendMessageW(self.get(LIGHT), CB_RESETCONTENT, 0, 0);
        for s in [
            "关闭",
            "常亮",
            "呼吸",
            "彩虹",
            "踏雪无痕",
            "雨中漫步",
            "涟漪",
            "繁星",
            "川流",
            "潮汐",
            "如影随形",
            "正弦波",
            "风车",
            "瀑布",
            "花开",
        ] {
            SendMessageW(self.get(LIGHT), CB_ADDSTRING, 0, wide(s).as_ptr() as isize);
        }
        SendMessageW(
            self.get(LIGHT),
            CB_SETCURSEL,
            self.lights
                .iter()
                .position(|v| Some(*v) == d.light)
                .unwrap_or(0),
            0,
        );
        self.set(
            self.get(DPI_EDIT),
            &d.dpi.map(|v| v.to_string()).unwrap_or_default(),
        );
    }
    unsafe fn render(&self) {
        for (i, d) in self.snapshot.devices.iter().enumerate() {
            let charge = match d.charging {
                Some(true) => "充电中",
                Some(false) => "未充电",
                None => "充电状态未知",
            };
            let link = if !d.present {
                "未连接"
            } else if d.wired {
                "USB 有线"
            } else {
                "2.4G 无线"
            };
            let age = if d.updated == 0 {
                "尚无有效读数".into()
            } else {
                format!("{} 秒前成功读取", backend::now().saturating_sub(d.updated))
            };
            self.set(
                self.cards[i],
                &if d.fresh {
                    format!("{link} · {charge}\r\n{age}\r\n{}", d.identity)
                } else {
                    format!(
                        "{link}\r\n{}\r\n{age}",
                        if d.percent.is_some() {
                            "上次电量，当前未确认"
                        } else {
                            "等待有效电量"
                        }
                    )
                },
            );
        }
        let m = self.selected();
        let d = &self.snapshot.devices[m];
        let enabled = d.fresh && self.pending.is_none();
        for id in [
            DPI_EDIT,
            DPI_APPLY,
            RATE,
            RATE_APPLY,
            SLEEP,
            SLEEP_APPLY,
            LIGHT,
            LIGHT_APPLY,
        ] {
            let supported = if [DPI_EDIT, DPI_APPLY].contains(&id) {
                m != 2 && d.dpi.is_some()
            } else if [LIGHT, LIGHT_APPLY].contains(&id) {
                m == 2 && d.light.is_some()
            } else if [RATE, RATE_APPLY].contains(&id) {
                d.rate.is_some()
            } else {
                d.sleep_seconds.is_some()
            };
            EnableWindow(self.get(id), (enabled && supported) as i32);
        }
        EnableWindow(self.get(REFRESH), self.pending.is_none() as i32);
        EnableWindow(self.get(DEVICE), self.pending.is_none() as i32);
        let detail = if !d.error.is_empty() {
            d.error.clone()
        } else if !d.settings_error.is_empty() {
            d.settings_error.clone()
        } else {
            format!(
                "当前：{} Hz · 休眠 {} 秒",
                d.rate.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                d.sleep_seconds
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into())
            )
        };
        self.set(self.detail, &detail);
    }
    unsafe fn begin(&mut self, hwnd: HWND, action: Option<(usize, Action)>) {
        if self.pending.is_some() {
            return;
        }
        self.writing = action.is_some();
        let snapshot = self.snapshot.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.set(
            self.status,
            if self.writing {
                "正在写入并读回核对…"
            } else {
                "正在读取设备…"
            },
        );
        self.render();
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = backend::refresh(&snapshot, action);
            if tx.send(result).is_ok() {
                PostMessageW(window as HWND, DONE, 0, 0);
            }
        });
    }
    unsafe fn complete(&mut self, hwnd: HWND) {
        let Some(rx) = self.pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok((mut snapshot, result)) => {
                snapshot.interval = self.snapshot.interval;
                snapshot.threshold = self.snapshot.threshold;
                snapshot.notices = self.snapshot.notices;
                self.snapshot = snapshot;
                let alerts = self.snapshot.alerts();
                self.trays(hwnd);
                for i in alerts {
                    self.balloon(hwnd, i);
                }
                let saved = self.snapshot.save();
                self.populate();
                self.render();
                InvalidateRect(hwnd, null(), 0);
                let online = self.snapshot.devices.iter().filter(|d| d.fresh).count();
                let s=match(result,saved){(Err(e),_)=>format!("操作未确认：{e}"),(_,Err(e))=>format!("已读取，但保存失败：{e}"),_=>format!("{} · {online}/3 台设备有有效读数 · 每 {} 秒刷新 · 关闭窗口后继续在托盘运行",if self.writing{"写入已读回确认"}else{"已更新"},self.snapshot.interval)};
                self.set(self.status, &s);
            }
            Err(e) => {
                self.set(self.status, &format!("后台读取异常：{e}"));
                self.render();
            }
        }
    }
    unsafe fn apply(&mut self, hwnd: HWND, id: usize) {
        let m = self.selected();
        let action = match id {
            DPI_APPLY => {
                let mut b = [0u16; 32];
                GetWindowTextW(self.get(DPI_EDIT), b.as_mut_ptr(), 32);
                let s =
                    String::from_utf16_lossy(&b[..b.iter().position(|v| *v == 0).unwrap_or(32)]);
                let max = if m == 0 { 30000 } else { 26000 };
                let min = if m == 0 { 100 } else { 50 };
                match s.parse::<u16>() {
                    Ok(v) if (min..=max).contains(&v) && v % 50 == 0 => Action::Dpi(v),
                    _ => {
                        message(&format!("请输入 {min}–{max} 范围内、50 的整数倍 DPI"));
                        return;
                    }
                }
            }
            RATE_APPLY => {
                let n = SendMessageW(self.get(RATE), CB_GETCURSEL, 0, 0);
                if n < 0 {
                    return;
                }
                Action::Rate(self.rates[n as usize])
            }
            SLEEP_APPLY => {
                let n = SendMessageW(self.get(SLEEP), CB_GETCURSEL, 0, 0);
                if n < 0 {
                    return;
                }
                Action::Sleep(self.sleeps[n as usize])
            }
            LIGHT_APPLY => {
                let n = SendMessageW(self.get(LIGHT), CB_GETCURSEL, 0, 0);
                if n < 0 {
                    return;
                }
                Action::Light(self.lights[n as usize])
            }
            _ => return,
        };
        self.begin(hwnd, Some((m, action)));
    }
    unsafe fn preferences(&mut self, hwnd: HWND) {
        let i = SendMessageW(self.get(INTERVAL), CB_GETCURSEL, 0, 0);
        let t = SendMessageW(self.get(THRESHOLD), CB_GETCURSEL, 0, 0);
        if i < 0 || t < 0 {
            return;
        }
        self.snapshot.interval = [30, 60, 120, 300, 600, 1800][i as usize];
        self.snapshot.threshold = [5, 10, 15, 20, 25, 30, 40, 50][t as usize];
        self.snapshot.notices =
            SendMessageW(self.get(NOTICES), BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
        SetTimer(hwnd, 1, self.snapshot.interval * 1000, None);
        match self.snapshot.save() {
            Ok(()) => self.set(self.status, "偏好已保存"),
            Err(e) => self.set(self.status, &format!("保存失败：{e}")),
        }
    }
    unsafe fn diagnose(&self) {
        let p = backend::data_path().with_file_name("diagnostics.json");
        let r = crate::hid::enumerate()
            .and_then(|v| serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()))
            .and_then(|v| {
                std::fs::create_dir_all(p.parent().unwrap())
                    .and_then(|_| std::fs::write(&p, v))
                    .map_err(|e| e.to_string())
            });
        match r {
            Ok(()) => message(&format!("诊断已保存：{}", p.display())),
            Err(e) => message(&format!("导出失败：{e}")),
        }
    }
    unsafe fn trays(&mut self, hwnd: HWND) {
        for (i,name) in NAMES.iter().enumerate() {
            let d = &self.snapshot.devices[i];
            let label = d
                .percent
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into());
            let icon = number_icon(
                &label,
                if d.fresh {
                    if i == 0 {
                        GREEN
                    } else if i == 1 {
                        ACCENT
                    } else {
                        0x00c67882
                    }
                } else {
                    MUTED
                },
            );
            let mut n: NOTIFYICONDATAW = zeroed();
            n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            n.hWnd = hwnd;
            n.uID = i as u32 + 1;
            n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            n.uCallbackMessage = TRAY;
            n.hIcon = icon;
            copy_wide(
                &mut n.szTip,
                &format!(
                    "{}：{}% · {}{}",
                    name,
                    d.percent
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "未知".into()),
                    if d.fresh {
                        "最新读数"
                    } else {
                        "未连接/缓存"
                    },
                    if d.charging == Some(true) {
                        " · 充电中"
                    } else {
                        ""
                    }
                ),
            );
            if Shell_NotifyIconW(
                if self.tray_added[i] {
                    NIM_MODIFY
                } else {
                    NIM_ADD
                },
                &n,
            ) != 0
            {
                self.tray_added[i] = true;
                if !self.tray_icons[i].is_null() {
                    DestroyIcon(self.tray_icons[i]);
                }
                self.tray_icons[i] = icon;
            } else {
                DestroyIcon(icon);
                self.set(self.status, "系统托盘暂不可用，主窗口仍可读取设备");
            }
        }
    }
    unsafe fn balloon(&self, hwnd: HWND, i: usize) {
        let mut n: NOTIFYICONDATAW = zeroed();
        n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = hwnd;
        n.uID = i as u32 + 1;
        n.uFlags = NIF_INFO;
        n.dwInfoFlags = NIIF_WARNING;
        copy_wide(&mut n.szInfoTitle, &format!("{} 电量偏低", NAMES[i]));
        copy_wide(
            &mut n.szInfo,
            &format!(
                "当前电量 {}%，请及时充电。",
                self.snapshot.devices[i].percent.unwrap()
            ),
        );
        Shell_NotifyIconW(NIM_MODIFY, &n);
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let target = BeginPaint(hwnd, &mut ps);
        let mut r = zeroed();
        GetClientRect(hwnd, &mut r);
        let dc = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, r.right, r.bottom);
        let old = SelectObject(dc, bitmap);
        fill(dc, r, BG);
        SetMapMode(dc, MM_ANISOTROPIC);
        SetWindowExtEx(dc, 96, 96, null_mut());
        SetViewportExtEx(dc, self.dpi as i32, self.dpi as i32, null_mut());
        let width = r.right * 96 / self.dpi as i32;
        let cw = (width - 80) / 3;
        text(dc, "Peripheral Desk", rect(32, 23, 440, 44), 28, 700, INK);
        text(
            dc,
            "外设控制台 / 电量、性能与日常设置",
            rect(34, 70, 650, 24),
            13,
            400,
            MUTED,
        );
        for (i,name) in NAMES.iter().enumerate() {
            let x = 32 + i as i32 * (cw + 16);
            round(dc, rect(x, 116, cw, 188), 0xffffff, BORDER);
            let d = &self.snapshot.devices[i];
            let color = if !d.fresh {
                MUTED
            } else if i == 0 {
                GREEN
            } else if i == 1 {
                ACCENT
            } else {
                0x00c67882
            };
            text(
                dc,
                if i == 0 {
                    "RAZER / MOUSE"
                } else if i == 1 {
                    "MCHOSE / MOUSE"
                } else {
                    "MCHOSE / KEYBOARD"
                },
                rect(x + 18, 131, cw - 95, 18),
                11,
                600,
                color,
            );
            text(dc, name, rect(x + 18, 155, cw - 85, 26), 17, 600, INK);
            text(
                dc,
                &d.percent
                    .map(|v| format!("{v}%"))
                    .unwrap_or_else(|| "—".into()),
                rect(x + cw - 89, 137, 78, 40),
                27,
                700,
                color,
            );
            fill(dc, rect(x + 18, 281, cw - 36, 5), BORDER);
            if let Some(p) = d.percent {
                fill(dc, rect(x + 18, 281, (cw - 36) * p as i32 / 100, 5), color);
            }
        }
        round(dc, rect(32, 324, width - 64, 229), 0xffffff, BORDER);
        text(dc, "设备设置", rect(48, 327, 240, 23), 12, 600, MUTED);
        let col = (width - 128) / 4;
        for (i, label) in ["DPI · X/Y 同步", "回报率", "自动休眠", "键盘灯效"]
            .iter()
            .enumerate()
        {
            text(
                dc,
                label,
                rect(48 + i as i32 * (col + 16), 420, col, 24),
                13,
                600,
                INK,
            );
        }
        text(
            dc,
            "点击应用保存到设备。每次只更改选中的设置，鼠标 DPI 同步应用于 X/Y 轴。",
            rect(48, 532, width - 96, 18),
            11,
            400,
            MUTED,
        );
        text(dc, "工具偏好", rect(34, 571, 300, 25), 17, 600, INK);
        text(dc, "自动刷新", rect(36, 615, 100, 24), 13, 400, INK);
        text(dc, "低电阈值", rect(274, 615, 100, 24), 13, 400, INK);
        text(
            dc,
            "托盘单击打开面板 · 右键刷新或退出 · 电量灰色表示历史值",
            rect(34, 653, width - 68, 21),
            12,
            400,
            MUTED,
        );
        SetMapMode(dc, MM_TEXT);
        BitBlt(target, 0, 0, r.right, r.bottom, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
    unsafe fn draw_button(&self, item: &DRAWITEMSTRUCT) {
        let disabled = item.itemState & ODS_DISABLED != 0;
        let color = if disabled {
            0x00efeeec
        } else if item.itemState & ODS_SELECTED != 0 {
            0x00eadbc9
        } else {
            0x00f9f2e8
        };
        round(
            item.hDC,
            item.rcItem,
            color,
            if disabled { BORDER } else { 0x00d7baa1 },
        );
        let mut b = [0u16; 80];
        GetWindowTextW(item.hwndItem, b.as_mut_ptr(), 80);
        SetBkMode(item.hDC, TRANSPARENT as i32);
        SetTextColor(item.hDC, if disabled { MUTED } else { INK });
        let old = SelectObject(item.hDC, self.font);
        let mut r = item.rcItem;
        DrawTextW(
            item.hDC,
            b.as_ptr(),
            -1,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(item.hDC, old);
        if item.itemState & ODS_FOCUS != 0 {
            let mut r = item.rcItem;
            InflateRect(&mut r, -4, -4);
            DrawFocusRect(item.hDC, &r);
        }
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}
unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(dc, &r, b);
    DeleteObject(b);
}
unsafe fn round(dc: HDC, r: RECT, color: u32, border: u32) {
    let b = CreateSolidBrush(color);
    let p = CreatePen(PS_SOLID, 1, border);
    let ob = SelectObject(dc, b);
    let op = SelectObject(dc, p);
    RoundRect(dc, r.left, r.top, r.right, r.bottom, 12, 12);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b);
    DeleteObject(p);
}
unsafe fn font(size: i32, weight: i32) -> HFONT {
    CreateFontW(
        size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Microsoft YaHei UI").as_ptr(),
    )
}
unsafe fn text(dc: HDC, s: &str, mut r: RECT, size: i32, weight: i32, color: u32) {
    let f = font(-size, weight);
    let old = SelectObject(dc, f);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    DrawTextW(
        dc,
        wide(s).as_ptr(),
        -1,
        &mut r,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
    SelectObject(dc, old);
    DeleteObject(f);
}
fn copy_wide<const N: usize>(dest: &mut [u16; N], s: &str) {
    for (d, c) in dest[..N - 1].iter_mut().zip(s.encode_utf16()) {
        *d = c;
    }
}
unsafe fn number_icon(label: &str, color: u32) -> HICON {
    let dc = CreateCompatibleDC(null_mut());
    let mut info: BITMAPINFO = zeroed();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = 32;
    info.bmiHeader.biHeight = -32;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let mut pixels = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
    if bitmap.is_null() {
        DeleteDC(dc);
        return CopyIcon(LoadIconW(null_mut(), IDI_APPLICATION));
    }
    let old = SelectObject(dc, bitmap);
    fill(dc, rect(0, 0, 32, 32), color);
    let f = font(if label.len() > 2 { -20 } else { -26 }, 700);
    let of = SelectObject(dc, f);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, 0xffffff);
    let mut r = rect(0, 0, 32, 32);
    DrawTextW(
        dc,
        wide(label).as_ptr(),
        -1,
        &mut r,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    GdiFlush();
    for p in std::slice::from_raw_parts_mut(pixels as *mut u32, 1024) {
        *p |= 0xff000000;
    }
    SelectObject(dc, of);
    DeleteObject(f);
    SelectObject(dc, old);
    let bits = [0u8; 128];
    let mask = CreateBitmap(32, 32, 1, 1, bits.as_ptr() as _);
    let icon = CreateIconIndirect(&ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: bitmap,
    });
    DeleteObject(mask);
    DeleteObject(bitmap);
    DeleteDC(dc);
    icon
}
