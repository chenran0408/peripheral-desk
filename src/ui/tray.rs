//! Tray icon, right-click menu and the battery flyout window.
use super::{
    pages::{connection, system_reading},
    widgets::{HOVER, PRESSED, WARN},
    App, Command, Page, OPEN, TRAY,
};
use crate::{
    devices::{Kind, REGISTRY},
    hid::wide,
    paint::{Canvas, R, *},
};
use std::ptr::{null, null_mut};
use std::{
    mem::{size_of, zeroed},
    time::Instant,
};

/// Flyout animation state owned by the tray rework.
#[derive(Default)]
pub(crate) struct TrayFx {
    opening: Option<Instant>,
    closing: Option<Instant>,
    target: RECT,
    rows_started: Option<Instant>,
    rings_started: usize,
    scroll_offset: i32,
    scroll_drag: Option<(i32, i32)>,
}

fn scroll_metrics(app: &App, height: i32) -> (i32, i32, i32, i32, i32) {
    let count = REGISTRY.len() + app.snapshot.system_devices.len();
    let sections =
        i32::from(!REGISTRY.is_empty()) + i32::from(!app.snapshot.system_devices.is_empty());
    let content_height = sections * 22 + count as i32 * 64;
    let list_top = 58;
    let footer = height - 50;
    let viewport_height = footer - list_top;
    let max_offset = (content_height - viewport_height).max(0);
    let track_top = list_top + 18;
    let track_bottom = footer - 18;
    let track_height = track_bottom - track_top;
    let thumb_height = if max_offset == 0 {
        track_height
    } else {
        (track_height * viewport_height / content_height).clamp(24, track_height)
    };
    (list_top, footer, max_offset, track_top, thumb_height)
}

impl TrayFx {
    pub(crate) fn tick(&mut self, hwnd: HWND) -> bool {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if ptr.is_null() {
                return false;
            }
            let app = &mut *ptr;
            let flyout = app.flyout;
            if flyout.is_null() || IsWindow(flyout) == 0 {
                self.opening = None;
                self.closing = None;
                return false;
            }
            if let Some(start) = self.rows_started {
                let elapsed = start.elapsed().as_millis() as usize;
                let total = REGISTRY.len() + app.snapshot.system_devices.len();
                while self.rings_started < total && elapsed >= self.rings_started * 60 {
                    let row = self.rings_started;
                    let percent = if row < REGISTRY.len() {
                        let state = &app.snapshot.devices[row];
                        state.fresh.then_some(state.percent).flatten()
                    } else {
                        app.snapshot.system_devices[row - REGISTRY.len()]
                            .state
                            .percent
                    };
                    app.fx.widget.ring_target(0x2000 + row as u64, percent);
                    self.rings_started += 1;
                }
            }
            if let Some(start) = self.opening {
                let t = start.elapsed().as_millis() as f32 / 200.;
                if t >= 1. {
                    SetWindowPos(
                        flyout,
                        HWND_TOPMOST,
                        self.target.left,
                        self.target.top,
                        self.target.right - self.target.left,
                        self.target.bottom - self.target.top,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    SetLayeredWindowAttributes(flyout, 0, 255, LWA_ALPHA);
                    self.opening = None;
                } else {
                    let eased = 1. - (1. - t).powi(3);
                    let y = self.target.top + (12. * (1. - eased)) as i32;
                    SetWindowPos(
                        flyout,
                        HWND_TOPMOST,
                        self.target.left,
                        y,
                        self.target.right - self.target.left,
                        self.target.bottom - self.target.top,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    SetLayeredWindowAttributes(flyout, 0, (eased * 255.) as u8, LWA_ALPHA);
                }
                InvalidateRect(flyout, null(), 0);
            }
            if let Some(start) = self.closing {
                let t = start.elapsed().as_millis() as f32 / 120.;
                if t >= 1. {
                    ShowWindow(flyout, SW_HIDE);
                    SetLayeredWindowAttributes(flyout, 0, 255, LWA_ALPHA);
                    self.opening = None;
                    self.closing = None;
                    self.rows_started = None;
                    self.rings_started = 0;
                    return false;
                }
                SetLayeredWindowAttributes(flyout, 0, ((1. - t) * 255.) as u8, LWA_ALPHA);
                InvalidateRect(flyout, null(), 0);
            }
            self.opening.is_some()
                || self.closing.is_some()
                || self.rings_started < REGISTRY.len() + app.snapshot.system_devices.len()
                || app.fx.widget.is_active()
        }
    }
}
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

pub(super) unsafe extern "system" fn flyout_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
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
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let a = &mut *ptr;
    match msg {
        WM_ACTIVATE if (w & 0xffff) == WA_INACTIVE as usize => {
            if a.fx.flyout.closing.is_none() {
                a.fx.flyout.closing = Some(Instant::now());
                a.fx_wake(main);
            }
            0
        }
        WM_KILLFOCUS => {
            if a.fx.flyout.closing.is_none() {
                a.fx.flyout.closing = Some(Instant::now());
                a.fx_wake(main);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            {
                let c = Canvas::new(dc, a.dpi);
                let mut bounds: RECT = zeroed();
                GetClientRect(hwnd, &mut bounds);
                let height = a.logical(bounds.bottom);
                let count = REGISTRY.len() + a.snapshot.system_devices.len();
                let hids = REGISTRY.len();
                let (list_top, footer, max_offset, track_top, thumb_height) =
                    scroll_metrics(a, height);
                let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
                let mut y = list_top - scroll;
                c.rect(R::new(0, 0, 360, height), PANEL);
                c.text("外设盒", R::new(20, 13, 205, 25), 17, 600, TEXT, DT_LEFT);
                c.text(
                    &format!("{count} 台设备"),
                    R::new(220, 16, 120, 20),
                    11,
                    400,
                    DIM,
                    DT_RIGHT,
                );
                c.line(18, 48, 342, 48, BORDER, 1.);
                let clip = SaveDC(dc);
                IntersectClipRect(dc, 0, a.px(list_top), a.px(360), a.px(footer));
                if count == 0 {
                    c.text(
                        "未安装或尚未加载设备驱动",
                        R::new(20, 90, 320, 25),
                        14,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "到主页面添加 · 安装后自动重启生效",
                        R::new(20, 118, 320, 22),
                        11,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                }
                if hids > 0 {
                    c.text("我的设备", R::new(20, y, 320, 18), 10, 600, DIM, DT_LEFT);
                    y += 22;
                }
                for (i, spec) in REGISTRY.iter().enumerate() {
                    let s = &a.snapshot.devices[i];
                    let key = 0x2000 + i as u64;
                    if a.fx.widget.hover(key) {
                        c.capsule(R::new(9, y, 326, 60), HOVER, None);
                    }
                    c.image(a.images.images[i], R::new(19, y + 7, 40, 44), s.fresh);
                    c.text(
                        spec.name,
                        R::new(68, y + 4, 190, 22),
                        13,
                        500,
                        if s.fresh { TEXT } else { DIM },
                        DT_LEFT,
                    );
                    c.text(
                        connection(s, a.images.installed[i]),
                        R::new(68, y + 28, 205, 19),
                        11,
                        400,
                        DIM,
                        DT_LEFT,
                    );
                    if s.fresh && !matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                        let percent = a.fx.widget.ring_value(key).unwrap_or(0.).round() as u8;
                        c.battery_icon(
                            R::new(288, y + 10, 34, 34),
                            percent,
                            s.charging == Some(true),
                            if percent < 20 { WARN } else { GREEN },
                        );
                        c.text(
                            &format!("{percent}%"),
                            R::new(260, y + 40, 90, 16),
                            10,
                            500,
                            TEXT,
                            DT_CENTER,
                        );
                    } else if s.fresh {
                        c.text(
                            "USB",
                            R::new(265, y + 10, 78, 30),
                            14,
                            500,
                            SUCCESS,
                            DT_RIGHT,
                        );
                    } else {
                        c.text(
                            "未连接",
                            R::new(260, y + 12, 82, 26),
                            11,
                            400,
                            DIM,
                            DT_RIGHT,
                        );
                    }
                    y += 64;
                }
                if !a.snapshot.system_devices.is_empty() {
                    c.text("系统设备", R::new(20, y, 320, 18), 10, 600, DIM, DT_LEFT);
                    y += 22;
                }
                for (i, device) in a.snapshot.system_devices.iter().enumerate() {
                    let index = REGISTRY.len() + i;
                    let key = 0x2000 + index as u64;
                    if a.fx.widget.hover(key) {
                        c.capsule(R::new(9, y, 326, 60), HOVER, None);
                    }
                    c.text(
                        &device.name,
                        R::new(22, y + 5, 235, 22),
                        13,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "Windows · 只读",
                        R::new(22, y + 30, 220, 18),
                        11,
                        400,
                        DIM,
                        DT_LEFT,
                    );
                    if let Some(percent) = device.state.percent {
                        let shown =
                            a.fx.widget
                                .ring_value(key)
                                .unwrap_or(percent as f32)
                                .round() as u8;
                        c.battery_icon(
                            R::new(288, y + 10, 34, 34),
                            shown,
                            device.state.charging == Some(true),
                            if shown < 20 { WARN } else { GREEN },
                        );
                        c.text(
                            &format!("{shown}%"),
                            R::new(260, y + 40, 90, 16),
                            10,
                            500,
                            TEXT,
                            DT_CENTER,
                        );
                    } else {
                        c.text(
                            &system_reading(device),
                            R::new(248, y + 13, 95, 26),
                            11,
                            400,
                            DIM,
                            DT_RIGHT,
                        );
                    }
                    y += 64;
                }
                RestoreDC(dc, clip);
                if max_offset > 0 {
                    let track_height = footer - 18 - track_top;
                    let thumb_y = track_top + (track_height - thumb_height) * scroll / max_offset;
                    c.chevron(
                        R::new(343, list_top + 3, 12, 12),
                        ChevronDirection::Up,
                        if scroll > 0 { DIM } else { BORDER },
                        1.2,
                    );
                    c.chevron(
                        R::new(343, footer - 15, 12, 12),
                        ChevronDirection::Down,
                        if scroll < max_offset { DIM } else { BORDER },
                        1.2,
                    );
                    c.capsule(R::new(347, track_top, 3, track_height), BORDER, None);
                    c.capsule(
                        R::new(343, thumb_y, 11, thumb_height),
                        if a.fx.flyout.scroll_drag.is_some() {
                            RED
                        } else {
                            DIM
                        },
                        None,
                    );
                }
                c.line(18, footer, 342, footer, BORDER, 1.);
                let labels = ["打开主窗口", "重新读取", "退出"];
                let widths = [112, 112, 112];
                let mut x = 12;
                for (i, label) in labels.iter().enumerate() {
                    let key = 0x2100 + i as u64;
                    let color = if a.fx.widget.pressed(key) {
                        PRESSED
                    } else if a.fx.widget.hover(key) {
                        HOVER
                    } else {
                        PANEL
                    };
                    c.capsule(R::new(x, footer + 8, widths[i], 32), color, None);
                    c.text(
                        label,
                        R::new(x, footer + 8, widths[i], 32),
                        11,
                        500,
                        if i == 2 { RED } else { TEXT },
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    x += widths[i] + 4;
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONUP => {
            let x = a.logical(l as i16 as i32);
            let y = a.logical((l >> 16) as i16 as i32);
            if a.fx.flyout.scroll_drag.take().is_some() {
                ReleaseCapture();
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            let height = a.logical(rc.bottom);
            let (list_top, footer, max_offset, _, _) = scroll_metrics(a, height);
            if x >= 338 && y >= list_top && y < footer {
                return 0;
            }
            let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
            let content_y = y + scroll;
            let mut row_y = 80;
            let mut device = None;
            for i in 0..REGISTRY.len() {
                if (9..=335).contains(&x) && content_y >= row_y && content_y < row_y + 64 {
                    device = Some(i + 1);
                    break;
                }
                row_y += 64;
            }
            if device.is_none() && !a.snapshot.system_devices.is_empty() {
                let sys_y = 80 + REGISTRY.len() as i32 * 64;
                let sys_y = sys_y + i32::from(!REGISTRY.is_empty()) * 22;
                let idx = ((content_y - sys_y) / 64).max(0) as usize;
                if (9..=335).contains(&x)
                    && content_y >= sys_y
                    && idx < a.snapshot.system_devices.len()
                {
                    device = Some(0);
                }
            }
            let action = if y >= footer + 8 && y <= footer + 40 {
                match x {
                    12..=124 => Some(0),
                    128..=240 => Some(1),
                    244..=356 => Some(2),
                    _ => None,
                }
            } else {
                None
            };
            a.fx.widget.clear_press();
            if let Some(action) = action {
                match action {
                    0 => {
                        a.fx.flyout.closing = Some(Instant::now());
                        PostMessageW(main, OPEN, 0, 0);
                    }
                    1 => {
                        a.fx.flyout.closing = Some(Instant::now());
                        a.begin(main, None);
                    }
                    _ => a.quit(main),
                }
                a.fx_wake(main);
            } else {
                a.fx.flyout.closing = Some(Instant::now());
                PostMessageW(main, OPEN, device.unwrap_or(0), 0);
                a.fx_wake(main);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let x = a.logical(l as i16 as i32);
            let y = a.logical((l >> 16) as i16 as i32);
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            let height = a.logical(rect.bottom);
            let (list_top, footer, max_offset, track_top, thumb_height) = scroll_metrics(a, height);
            if max_offset > 0 && x >= 338 && y >= list_top && y < footer {
                let track_height = footer - 18 - track_top;
                let thumb_y = track_top
                    + (track_height - thumb_height)
                        * a.fx.flyout.scroll_offset.clamp(0, max_offset)
                        / max_offset;
                if y < list_top + 18 {
                    a.fx.flyout.scroll_offset =
                        (a.fx.flyout.scroll_offset - 64).clamp(0, max_offset);
                } else if y >= footer - 18 {
                    a.fx.flyout.scroll_offset =
                        (a.fx.flyout.scroll_offset + 64).clamp(0, max_offset);
                } else if (thumb_y..thumb_y + thumb_height).contains(&y) {
                    a.fx.flyout.scroll_drag = Some((y, a.fx.flyout.scroll_offset));
                    SetCapture(hwnd);
                } else {
                    let thumb_top = y - thumb_height / 2;
                    let travel = track_height - thumb_height;
                    a.fx.flyout.scroll_offset =
                        ((thumb_top - track_top).clamp(0, travel) * max_offset / travel)
                            .clamp(0, max_offset);
                }
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if y >= footer + 8 && y <= footer + 40 {
                let index = ((x - 12) / 116).clamp(0, 2) as u64;
                a.fx.widget.press(0x2100 + index);
                InvalidateRect(hwnd, null(), 0);
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
            let x = a.logical(l as i16 as i32);
            let y = a.logical((l >> 16) as i16 as i32);
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            let height = a.logical(rc.bottom);
            let (list_top, footer, max_offset, track_top, thumb_height) = scroll_metrics(a, height);
            if let Some((start_y, start_offset)) = a.fx.flyout.scroll_drag {
                let track_height = footer - 18 - track_top;
                let travel = track_height - thumb_height;
                let delta = (y - start_y) * max_offset / travel;
                a.fx.flyout.scroll_offset = (start_offset + delta).clamp(0, max_offset);
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
            let mut row_y = 80 - scroll;
            for i in 0..REGISTRY.len() + a.snapshot.system_devices.len() {
                let key = 0x2000 + i as u64;
                a.fx.widget.set_hover(
                    key,
                    (9..=335).contains(&x)
                        && (list_top..footer).contains(&y)
                        && y >= row_y
                        && y < row_y + 60,
                );
                row_y += 64;
                if i + 1 == REGISTRY.len() && !a.snapshot.system_devices.is_empty() {
                    row_y += 22;
                }
            }
            for i in 0..3 {
                let left = 12 + i * 116;
                a.fx.widget.set_hover(
                    0x2100 + i as u64,
                    x >= left && x < left + 112 && y >= footer + 8 && y < footer + 40,
                );
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            let height = a.logical(rc.bottom);
            let (_, _, max_offset, _, _) = scroll_metrics(a, height);
            let delta = ((w >> 16) as i16 as i32) * 64 / WHEEL_DELTA as i32;
            a.fx.flyout.scroll_offset = (a.fx.flyout.scroll_offset - delta).clamp(0, max_offset);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        0x02a3 => {
            for i in 0..REGISTRY.len() + a.snapshot.system_devices.len() {
                a.fx.widget.set_hover(0x2000 + i as u64, false);
            }
            for i in 0..3 {
                a.fx.widget.set_hover(0x2100 + i as u64, false);
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE as usize => {
            a.fx.flyout.closing = Some(Instant::now());
            a.fx_wake(main);
            0
        }
        WM_CLOSE => {
            a.fx.flyout.closing = Some(Instant::now());
            a.fx_wake(main);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
fn copy_wide<const N: usize>(dest: &mut [u16; N], text: &str) {
    for (d, c) in dest[..N - 1].iter_mut().zip(text.encode_utf16()) {
        *d = c;
    }
}
pub(super) unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..zeroed()
    }
}

/// Shared picker for the main device page and the EQ library.
pub(crate) unsafe fn pick_audio_component(
    hwnd: HWND,
) -> Result<Option<std::path::PathBuf>, String> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut file = [0u16; 32768];
    let filter = wide("C-Media 音效组件 (osConfLib.dll)\0osConfLib.dll\0\0");
    let title = wide("导入已安装的官方 x64 / osConfLib.dll（仅保存在本机）");
    let mut dialog: OPENFILENAMEW = zeroed();
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = hwnd;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
    dialog.lpstrTitle = title.as_ptr();
    if GetOpenFileNameW(&mut dialog) != 0 {
        let end = file.iter().position(|&v| v == 0).unwrap_or(file.len());
        Ok(Some(String::from_utf16_lossy(&file[..end]).into()))
    } else {
        let error = CommDlgExtendedError();
        if error == 0 {
            Ok(None)
        } else {
            Err(format!("选择音效组件失败：{error}"))
        }
    }
}

impl App {
    pub(super) unsafe fn tray(&mut self, hwnd: HWND) {
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
            .into_iter()
            .chain(
                self.snapshot
                    .system_devices
                    .iter()
                    .map(|d| format!("{} {}", d.name, system_reading(d))),
            )
            .collect::<Vec<_>>()
            .join("\n");
        copy_wide(
            &mut n.szTip,
            if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
                "外设盒 · 到主页面添加设备驱动"
            } else {
                &tip
            },
        );
        self.tray_added =
            Shell_NotifyIconW(if self.tray_added { NIM_MODIFY } else { NIM_ADD }, &n) != 0;
    }
    pub(super) unsafe fn notify(&self, hwnd: HWND, index: usize) {
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
    pub(super) unsafe fn menu(&mut self, hwnd: HWND) {
        let menu = CreatePopupMenu();
        for (i, spec) in REGISTRY.iter().enumerate() {
            let state = &self.snapshot.devices[i];
            let reading = if state.fresh && matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                "USB 供电".to_string()
            } else if state.fresh {
                state
                    .percent
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "电量未知".into())
            } else {
                "未连接".into()
            };
            AppendMenuW(
                menu,
                MF_GRAYED | MF_STRING,
                0,
                wide(&format!("{}　{}", spec.name, reading)).as_ptr(),
            );
        }
        for device in &self.snapshot.system_devices {
            AppendMenuW(
                menu,
                MF_GRAYED | MF_STRING,
                0,
                wide(&format!("{}　{}", device.name, system_reading(device))).as_ptr(),
            );
        }
        if !REGISTRY.is_empty() || !self.snapshot.system_devices.is_empty() {
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
        }
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
    pub(super) unsafe fn show_battery(&mut self, hwnd: HWND) {
        if !self.flyout.is_null() && IsWindowVisible(self.flyout) != 0 {
            self.fx.flyout.closing = Some(Instant::now());
            self.fx_wake(hwnd);
            return;
        }
        if self.flyout.is_null() {
            self.flyout = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
                wide("PeripheralDeskBattery").as_ptr(),
                wide("外设盒 · 设备电量").as_ptr(),
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
        let width = self.px(360);
        let count = REGISTRY.len() + self.snapshot.system_devices.len();
        let logical_height = 488;
        let height = self
            .px(logical_height)
            .min(area.bottom - area.top - self.px(24));
        let x = (point.x - width).clamp(area.left, area.right - width);
        let y = (point.y - height - 12).clamp(area.top, area.bottom - height);
        self.fx.flyout.target = RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        };
        self.fx.flyout.opening = Some(Instant::now());
        self.fx.flyout.closing = None;
        self.fx.flyout.rows_started = Some(Instant::now());
        self.fx.flyout.rings_started = 0;
        self.fx.flyout.scroll_offset = 0;
        self.fx.flyout.scroll_drag = None;
        for row in 0..count {
            self.fx.widget.ring_target(0x2000 + row as u64, Some(0));
        }
        SetLayeredWindowAttributes(self.flyout, 0, 0, LWA_ALPHA);
        SetWindowPos(
            self.flyout,
            HWND_TOPMOST,
            x,
            y + self.px(12),
            width,
            height,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        SetForegroundWindow(self.flyout);
        self.fx_wake(hwnd);
        InvalidateRect(self.flyout, null(), 0);
    }
}
