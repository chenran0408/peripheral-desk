//! Tray icon, right-click menu and the battery flyout window.
use super::{
    pages::{connection, system_reading},
    App, Command, Page, OPEN, TRAY,
};
use crate::{
    devices::{Kind, REGISTRY},
    hid::wide,
    paint::{Canvas, R, *},
};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub(super) unsafe extern "system" fn flyout_proc(
    hwnd: HWND,
    msg: u32,
    _w: WPARAM,
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
                let mut bounds: RECT = zeroed();
                GetClientRect(hwnd, &mut bounds);
                let height = a.logical(bounds.bottom);
                let visible = ((height - 112) / 66).max(0) as usize;
                c.rect(R::new(0, 0, 350, height), PANEL);
                c.text("设备电量", R::new(21, 16, 240, 32), 17, 600, TEXT, DT_LEFT);
                c.text("外设盒", R::new(21, 44, 280, 20), 10, 400, DIM, DT_LEFT);
                if REGISTRY.is_empty() && a.snapshot.system_devices.is_empty() {
                    c.text(
                        "未安装或尚未加载设备驱动",
                        R::new(21, 77, 308, 25),
                        14,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "到主页面添加 · 安装后自动重启生效",
                        R::new(21, 105, 308, 22),
                        11,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                }
                for (i, spec) in REGISTRY.iter().enumerate().take(visible) {
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
                for (i, device) in a
                    .snapshot
                    .system_devices
                    .iter()
                    .enumerate()
                    .take(visible.saturating_sub(REGISTRY.len()))
                {
                    let y = 76 + (REGISTRY.len() + i) as i32 * 66;
                    c.text(&device.name, R::new(22, y, 216, 25), 13, 500, TEXT, DT_LEFT);
                    c.text(
                        "Windows · 只读",
                        R::new(22, y + 25, 190, 19),
                        11,
                        400,
                        DIM,
                        DT_LEFT,
                    );
                    c.text(
                        &system_reading(device),
                        R::new(230, y + 1, 100, 40),
                        19,
                        400,
                        TEXT,
                        DT_RIGHT,
                    );
                }
                if REGISTRY.len() + a.snapshot.system_devices.len() > visible {
                    c.text(
                        "更多设备请在主页面查看",
                        R::new(20, height - 62, 310, 20),
                        11,
                        400,
                        DIM,
                        DT_CENTER,
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
            ShowWindow(self.flyout, SW_HIDE);
            return;
        }
        if self.flyout.is_null() {
            self.flyout = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
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
        let width = self.px(350);
        let height = self
            .px(112 + (REGISTRY.len() + self.snapshot.system_devices.len()).max(1) as i32 * 66)
            .min(area.bottom - area.top - self.px(24));
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
