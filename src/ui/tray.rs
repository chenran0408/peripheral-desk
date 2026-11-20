//! Tray icon, right-click menu and the battery flyout window.
use super::{
    pages::{connection, reading, system_reading},
    widgets::{HOVER, PRESSED, WARN},
    App, Command, Page, TRAY,
};
use crate::{
    devices::REGISTRY,
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
    entries: Vec<Command>,
    scroll_offset: i32,
    scroll_drag: Option<(i32, i32)>,
    hovered: Option<HoverTarget>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    Row(usize),
    Footer(usize),
}

fn hover_target(
    x: i32,
    y: i32,
    footer: i32,
    scroll: i32,
    installed: usize,
    system: usize,
) -> Option<HoverTarget> {
    if (footer + 8..footer + 40).contains(&y) && (12..348).contains(&x) {
        let offset = x - 12;
        return (offset % 172 < 164).then_some(HoverTarget::Footer((offset / 172) as usize));
    }
    if !(9..335).contains(&x) || !(58..footer).contains(&y) {
        return None;
    }
    let mut row_y = y + scroll - 80;
    for (offset, count) in [(0, installed), (installed, system)] {
        if count == 0 {
            continue;
        }
        if (0..64 * count as i32).contains(&row_y) {
            return (row_y % 64 < 60).then_some(HoverTarget::Row(offset + (row_y / 64) as usize));
        }
        row_y -= 64 * count as i32 + 22;
    }
    None
}

fn scroll_metrics(app: &App, height: i32) -> (i32, i32, i32, i32, i32) {
    let (installed, system) = app.fx.flyout.section_counts();
    let count = installed + system;
    let sections = i32::from(installed > 0) + i32::from(system > 0);
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
    fn section_counts(&self) -> (usize, usize) {
        let installed = self
            .entries
            .partition_point(|e| matches!(e, Command::Battery(_) | Command::UnitBattery(_)));
        (installed, self.entries.len() - installed)
    }

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
                    return false;
                }
                SetLayeredWindowAttributes(flyout, 0, ((1. - t) * 255.) as u8, LWA_ALPHA);
                InvalidateRect(flyout, null(), 0);
            }
            self.opening.is_some() || self.closing.is_some()
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
            let dest = BeginPaint(hwnd, &mut ps);
            let mut bounds: RECT = zeroed();
            GetClientRect(hwnd, &mut bounds);
            let dc = CreateCompatibleDC(dest);
            let bitmap = CreateCompatibleBitmap(dest, bounds.right, bounds.bottom);
            let previous = SelectObject(dc, bitmap);
            {
                let c = Canvas::new(dc, a.dpi);
                let height = a.logical(bounds.bottom);
                let entries = &a.fx.flyout.entries;
                let (hids, system) = a.fx.flyout.section_counts();
                let count = entries.len();
                let (list_top, footer, max_offset, track_top, thumb_height) =
                    scroll_metrics(a, height);
                let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
                let mut y = list_top - scroll;
                windows_sys::Win32::Graphics::GdiPlus::GdipGraphicsClear(c.graphics, PANEL);
                c.text("设备电量", R::new(20, 13, 205, 25), 17, 600, TEXT, DT_LEFT);
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
                        "暂无可显示电量的设备",
                        R::new(20, 90, 320, 25),
                        14,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "连接无线设备，或在插件管理中安装对应型号。",
                        R::new(20, 118, 320, 22),
                        11,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                }
                if hids > 0 {
                    c.text("外设", R::new(20, y, 320, 18), 10, 600, DIM, DT_LEFT);
                    y += 22;
                }
                for (row, &entry) in entries[..hids].iter().enumerate() {
                    let Some((i, name, s)) = a.unit_view(entry) else {
                        continue;
                    };
                    let spec = &REGISTRY[i];
                    if a.fx.flyout.hovered == Some(HoverTarget::Row(row)) {
                        c.capsule(R::new(9, y, 326, 60), HOVER, None);
                    }
                    c.image(a.images.images[i], R::new(19, y + 7, 40, 44), s.fresh);
                    c.text(
                        name,
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
                    if let Some(percent) = s.percent.filter(|_| s.fresh) {
                        let fill = if matches!(entry, Command::UnitBattery(_)) {
                            percent as f32
                        } else {
                            a.fx.widget
                                .ring_value(0x1000 + i as u64)
                                .unwrap_or(percent as f32)
                        };
                        c.battery_icon(
                            R::new(288, y + 10, 34, 34),
                            fill.round() as u8,
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
                    } else {
                        c.text(
                            &reading(s, spec),
                            R::new(260, y + 12, 82, 26),
                            11,
                            400,
                            DIM,
                            DT_RIGHT,
                        );
                    }
                    y += 64;
                }
                if system > 0 {
                    c.text("系统设备", R::new(20, y, 320, 18), 10, 600, DIM, DT_LEFT);
                    y += 22;
                }
                for (row, &entry) in entries[hids..].iter().enumerate() {
                    let Command::System(i) = entry else { continue };
                    let device = &a.snapshot.system_devices[i];
                    if a.fx.flyout.hovered == Some(HoverTarget::Row(hids + row)) {
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
                        let fill =
                            a.fx.widget
                                .ring_value(0x8000 + i as u64)
                                .unwrap_or(percent as f32);
                        c.battery_icon(
                            R::new(288, y + 10, 34, 34),
                            fill.round() as u8,
                            device.state.charging == Some(true),
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
                let labels = ["电量总览", "刷新电量"];
                for (i, label) in labels.iter().enumerate() {
                    let rect = R::new(12 + i as i32 * 172, footer + 8, 164, 32);
                    let key = 0x2100 + i as u64;
                    let color = if a.fx.widget.pressed(key) {
                        PRESSED
                    } else if a.fx.flyout.hovered == Some(HoverTarget::Footer(i)) {
                        HOVER
                    } else {
                        PANEL
                    };
                    c.capsule(rect, color, None);
                    c.text(
                        label,
                        rect,
                        11,
                        500,
                        TEXT,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                }
            }
            BitBlt(dest, 0, 0, bounds.right, bounds.bottom, dc, 0, 0, SRCCOPY);
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            DeleteDC(dc);
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONUP => {
            if focus_active_dialog(main) {
                a.fx.flyout.closing = Some(Instant::now());
                a.fx_wake(main);
                return 0;
            }
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
            let (_, footer, max_offset, _, _) = scroll_metrics(a, height);
            let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
            let (installed, system) = a.fx.flyout.section_counts();
            let target = hover_target(x, y, footer, scroll, installed, system);
            a.fx.widget.clear_press();
            match target {
                Some(HoverTarget::Row(row)) => {
                    let page = match a.fx.flyout.entries[row] {
                        Command::UnitBattery(i) => {
                            let Some(model) = a.select_unit(main, i) else {
                                return 0;
                            };
                            Page::Battery(model)
                        }
                        Command::Battery(model) => Page::Battery(model),
                        Command::System(index) => Page::System(index),
                        _ => return 0,
                    };
                    a.open_tray_page(main, page);
                }
                Some(HoverTarget::Footer(0)) => a.open_tray_page(main, Page::Home),
                Some(HoverTarget::Footer(1)) => a.begin(main, None),
                _ => {}
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
            let (installed, system) = a.fx.flyout.section_counts();
            if let Some(HoverTarget::Footer(index)) =
                hover_target(x, y, footer, a.fx.flyout.scroll_offset, installed, system)
            {
                a.fx.widget.press(0x2100 + index as u64);
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
            let (_, footer, max_offset, track_top, thumb_height) = scroll_metrics(a, height);
            if let Some((start_y, start_offset)) = a.fx.flyout.scroll_drag {
                let track_height = footer - 18 - track_top;
                let travel = track_height - thumb_height;
                if travel <= 0 {
                    return 0;
                }
                let delta = (y - start_y) * max_offset / travel;
                let offset = (start_offset + delta).clamp(0, max_offset);
                if a.fx.flyout.scroll_offset != offset {
                    a.fx.flyout.scroll_offset = offset;
                    InvalidateRect(hwnd, null(), 0);
                }
                return 0;
            }
            let scroll = a.fx.flyout.scroll_offset.clamp(0, max_offset);
            let (installed, system) = a.fx.flyout.section_counts();
            let hovered = hover_target(x, y, footer, scroll, installed, system);
            if a.fx.flyout.hovered != hovered {
                a.fx.flyout.hovered = hovered;
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            let height = a.logical(rc.bottom);
            let (_, footer, max_offset, _, _) = scroll_metrics(a, height);
            let delta = ((w >> 16) as i16 as i32) * 64 / WHEEL_DELTA as i32;
            let offset = (a.fx.flyout.scroll_offset - delta).clamp(0, max_offset);
            if offset != a.fx.flyout.scroll_offset {
                a.fx.flyout.scroll_offset = offset;
                let mut cursor = POINT {
                    x: l as i16 as i32,
                    y: (l >> 16) as i16 as i32,
                };
                ScreenToClient(hwnd, &mut cursor);
                let (installed, system) = a.fx.flyout.section_counts();
                a.fx.flyout.hovered = hover_target(
                    a.logical(cursor.x),
                    a.logical(cursor.y),
                    footer,
                    offset,
                    installed,
                    system,
                );
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        windows_sys::Win32::UI::Controls::WM_MOUSELEAVE => {
            if a.fx.flyout.hovered.take().is_some() {
                InvalidateRect(hwnd, null(), 0);
            }
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

pub(super) unsafe fn focus_active_dialog(owner: HWND) -> bool {
    if IsWindowEnabled(owner) != 0 {
        return false;
    }
    let dialog = GetLastActivePopup(owner);
    if !dialog.is_null() && dialog != owner {
        SetForegroundWindow(dialog);
    }
    true
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
        let mut entries = self.battery_entries();
        entries.sort_by_key(|entry| matches!(entry, Command::System(_)));
        if entries != self.fx.flyout.entries {
            self.fx.flyout.entries = entries;
            self.fx.flyout.hovered = None;
        }
        let tip = self
            .fx
            .flyout
            .entries
            .iter()
            .filter_map(|entry| match *entry {
                Command::Battery(_) | Command::UnitBattery(_) => {
                    let (i, name, state) = self.unit_view(*entry)?;
                    Some(format!("{} {}", name, reading(state, &REGISTRY[i])))
                }
                Command::System(i) => {
                    let device = &self.snapshot.system_devices[i];
                    Some(format!("{} {}", device.name, system_reading(device)))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        copy_wide(
            &mut n.szTip,
            if tip.is_empty() {
                "外设盒 · 暂无电量设备"
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
        for (id, label) in [(1, "查看电量"), (2, "设备调节"), (3, "退出程序")] {
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
            1 => self.open_tray_page(hwnd, Page::Home),
            2 => self.open_tray_page(hwnd, Page::Drivers),
            3 => self.quit(hwnd),
            _ => {}
        }
        PostMessageW(hwnd, WM_NULL, 0, 0);
    }
    unsafe fn open_tray_page(&mut self, hwnd: HWND, page: Page) {
        if !self.flyout.is_null() && IsWindowVisible(self.flyout) != 0 {
            self.fx.flyout.closing = Some(Instant::now());
            self.fx_wake(hwnd);
        }
        self.navigate(hwnd, page);
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
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
        let (installed, system) = self.fx.flyout.section_counts();
        let sections = i32::from(installed > 0) + i32::from(system > 0);
        let logical_height =
            (108 + sections * 22 + (installed + system) as i32 * 64).clamp(208, 488);
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
        self.fx.flyout.scroll_offset = 0;
        self.fx.flyout.scroll_drag = None;
        self.fx.flyout.hovered = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_and_scrolled_rows_select_the_displayed_battery() {
        let flyout = TrayFx {
            entries: vec![Command::Battery(2), Command::Battery(5), Command::System(0)],
            ..Default::default()
        };
        let (installed, system) = flyout.section_counts();
        let hit = |y| match hover_target(20, y, 300, 64, installed, system) {
            Some(HoverTarget::Row(row)) => Some(flyout.entries[row]),
            _ => None,
        };
        assert_eq!(hit(80), Some(Command::Battery(5)));
        assert_eq!(hit(148), None); // System section heading
        assert_eq!(hit(168), Some(Command::System(0)));
        assert_eq!(hit(310), None); // Footer cannot also select a row
    }
}
