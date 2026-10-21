//! Modal native G2 Pro lighting editor. Hardware work stays off the UI thread.
use crate::{
    audio_lighting::{self, LightingConfig, LightingMode, LightingState},
    hid::wide,
    paint::{self, Canvas, R},
};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{Dialogs::*, *},
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{EnableWindow, SetActiveWindow},
        WindowsAndMessaging::*,
    },
};

const TOGGLE: usize = 101;
const STATIC: usize = 102;
const BREATHING: usize = 103;
const RAINBOW: usize = 104;
const COLOR: usize = 105;
const SLOWER: usize = 106;
const FASTER: usize = 107;
const APPLY: usize = 108;
const REFRESH: usize = 109;
const CLOSE: usize = 110;
struct Dialog {
    config: LightingConfig,
    actual_enabled: Option<bool>,
    saved: bool,
    buttons: Vec<(usize, HWND)>,
    font: HFONT,
    brush: HBRUSH,
    dpi: u32,
    status: String,
    error: bool,
    writing: bool,
    pending: Option<Receiver<Result<LightingState, String>>>,
    custom_colors: [u32; 16],
}
impl Dialog {
    unsafe fn update_buttons(&self) {
        for (id, button) in &self.buttons {
            EnableWindow(
                *button,
                i32::from(
                    self.pending.is_none() && (*id != APPLY || self.actual_enabled.is_some()),
                ),
            );
        }
    }
    unsafe fn begin(&mut self, hwnd: HWND, writing: bool) {
        if self.pending.is_some() {
            return;
        }
        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.writing = writing;
        self.error = false;
        self.status = if writing {
            "正在发送灯效，并核对设备开关…"
        } else {
            "正在读取设备灯光开关与本工具保存配置…"
        }
        .into();
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        thread::spawn(move || {
            let _ = tx.send(if writing {
                audio_lighting::apply(&config)
            } else {
                audio_lighting::read()
            });
        });
    }
    unsafe fn poll(&mut self, hwnd: HWND) {
        let result = match self.pending.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("灯效操作意外结束，请重新读取设备状态".into())
            }
            _ => return,
        };
        self.pending = None;
        match result {
            Ok(state) => {
                self.actual_enabled = Some(state.enabled);
                self.saved = state.saved.is_some();
                self.config = state.saved.unwrap_or_default();
                self.config.enabled = state.enabled;
                self.error = false;
                self.status = if self.writing {
                    "灯效已发送并保存；设备开关读回确认成功。模式与颜色不具备完整设备读回。".into()
                } else {
                    state.notice
                };
            }
            Err(e) => {
                self.status = e;
                self.error = true;
            }
        }
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn command(&mut self, hwnd: HWND, id: usize) {
        if self.pending.is_some() {
            return;
        }
        match id {
            CLOSE | 2 => {
                DestroyWindow(hwnd);
                return;
            }
            APPLY | 1 => {
                if self.actual_enabled.is_some() {
                    self.begin(hwnd, true);
                }
                return;
            }
            REFRESH => {
                self.begin(hwnd, false);
                return;
            }
            TOGGLE => self.config.enabled = !self.config.enabled,
            STATIC => self.config.mode = LightingMode::Static,
            BREATHING => self.config.mode = LightingMode::Breathing,
            RAINBOW => self.config.mode = LightingMode::Rainbow,
            SLOWER => self.config.speed = self.config.speed.saturating_sub(1).max(1),
            FASTER => self.config.speed = (self.config.speed + 1).min(10),
            COLOR => {
                let [r, g, b] = self.config.rgb;
                let mut choose: CHOOSECOLORW = zeroed();
                choose.lStructSize = size_of::<CHOOSECOLORW>() as u32;
                choose.hwndOwner = hwnd;
                choose.rgbResult = r as u32 | (g as u32) << 8 | (b as u32) << 16;
                choose.lpCustColors = self.custom_colors.as_mut_ptr();
                choose.Flags = CC_RGBINIT | CC_FULLOPEN;
                if ChooseColorW(&mut choose) == 0 {
                    return;
                }
                self.config.rgb = [
                    choose.rgbResult as u8,
                    (choose.rgbResult >> 8) as u8,
                    (choose.rgbResult >> 16) as u8,
                ];
            }
            _ => return,
        }
        self.error = false;
        self.status = "设置尚未发送，点击“应用灯效”生效。".into();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    fn selected(&self, id: usize) -> bool {
        match id {
            TOGGLE => self.config.enabled,
            STATIC => self.config.mode == LightingMode::Static,
            BREATHING => self.config.mode == LightingMode::Breathing,
            RAINBOW => self.config.mode == LightingMode::Rainbow,
            _ => false,
        }
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let canvas = Canvas::new(dc, self.dpi);
        canvas.rect(R::new(0, 0, 880, 580), paint::BG);
        canvas.gradient(R::new(0, 0, 880, 3), paint::RED, paint::ORANGE);
        canvas.text(
            "G2 Pro 灯光",
            R::new(28, 22, 600, 36),
            26,
            600,
            paint::TEXT,
            DT_LEFT,
        );
        canvas.text(
            "自定义耳罩光效 · 独立 HID 控制",
            R::new(28, 65, 700, 24),
            14,
            400,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.round(
            R::new(28, 112, 270, 346),
            16,
            paint::PANEL,
            Some(paint::BORDER),
        );
        let [r, g, b] = self.config.rgb;
        let rgb = 0xff000000 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        canvas.ellipse(R::new(66, 150, 194, 194), paint::BG);
        if self.config.mode == LightingMode::Rainbow && self.config.enabled {
            for i in 0..6 {
                canvas.arc(
                    R::new(78, 162, 170, 170),
                    i as f32 * 60.,
                    58.,
                    [
                        0xffff5050, 0xffffca4a, 0xff6ce58a, 0xff48d9ef, 0xff7a88ff, 0xffde75f0,
                    ][i],
                    10.,
                );
            }
        } else {
            canvas.arc(
                R::new(78, 162, 170, 170),
                0.,
                359.9,
                if self.config.enabled {
                    rgb
                } else {
                    paint::BORDER
                },
                10.,
            );
        }
        canvas.text(
            "H",
            R::new(110, 205, 104, 75),
            56,
            600,
            if self.config.enabled {
                paint::TEXT
            } else {
                paint::DIM
            },
            DT_CENTER,
        );
        canvas.text(
            "灯效示意",
            R::new(58, 361, 210, 24),
            14,
            600,
            paint::TEXT,
            DT_CENTER,
        );
        canvas.text(
            "预览不代表设备实测颜色",
            R::new(42, 396, 242, 23),
            12,
            400,
            paint::DIM,
            DT_CENTER,
        );
        canvas.round(
            R::new(320, 112, 532, 346),
            16,
            paint::PANEL,
            Some(paint::BORDER),
        );
        canvas.text(
            "灯光开关",
            R::new(344, 133, 250, 24),
            16,
            600,
            paint::TEXT,
            DT_LEFT,
        );
        canvas.text(
            "灯效模式",
            R::new(344, 195, 440, 24),
            14,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            "颜色",
            R::new(344, 287, 175, 24),
            14,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.round(R::new(440, 279, 42, 36), 8, rgb, Some(paint::BORDER));
        canvas.text(
            &format!("#{r:02X}{g:02X}{b:02X}"),
            R::new(494, 286, 160, 24),
            15,
            500,
            paint::TEXT,
            DT_LEFT,
        );
        canvas.text(
            if self.config.mode == LightingMode::Rainbow {
                "彩虹使用多色渐变，单色选择保留供其他模式使用"
            } else {
                "常亮和呼吸模式使用所选 RGB 颜色"
            },
            R::new(344, 326, 480, 24),
            11,
            400,
            paint::DIM,
            DT_LEFT,
        );
        canvas.text(
            "速度",
            R::new(344, 376, 160, 24),
            14,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            &format!("{} / 10", self.config.speed),
            R::new(600, 376, 100, 24),
            16,
            500,
            paint::TEXT,
            DT_CENTER,
        );
        let actual = match self.actual_enabled {
            Some(true) => "上次设备实读：灯光开启",
            Some(false) => "上次设备实读：灯光关闭",
            None => "尚未读到设备灯光开关",
        };
        canvas.text(
            actual,
            R::new(28, 476, 430, 23),
            13,
            500,
            if self.actual_enabled.is_some() {
                paint::GREEN
            } else {
                paint::MUTED
            },
            DT_LEFT,
        );
        canvas.text(
            if self.saved {
                "模式、颜色、速度来自本工具保存配置；外部驱动更改不会自动同步。"
            } else {
                "首次打开：模式与颜色为建议初始值，不代表设备当前配置。"
            },
            R::new(28, 505, 820, 23),
            12,
            400,
            paint::DIM,
            DT_LEFT,
        );
        canvas.text(
            &self.status,
            R::new(28, 541, 820, 28),
            12,
            400,
            if self.error { paint::RED } else { paint::MUTED },
            DT_LEFT,
        );
        drop(canvas);
        EndPaint(hwnd, &ps);
    }
}
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        return DefWindowProcW(hwnd, message, w, l);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let dialog = &mut *pointer;
    match message {
        WM_PAINT => {
            dialog.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DRAWITEM => {
            let draw = &*(l as *const DRAWITEMSTRUCT);
            if draw.CtlType != ODT_BUTTON {
                return 0;
            }
            // Clear every physical pixel, including antialiased rounded corners.
            let background = if (TOGGLE..=FASTER).contains(&(draw.CtlID as usize)) {
                paint::PANEL
            } else {
                paint::BG
            };
            let brush = CreateSolidBrush(
                ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
            );
            FillRect(draw.hDC, &draw.rcItem, brush);
            DeleteObject(brush);
            let canvas = Canvas::new(draw.hDC, dialog.dpi);
            let width = (draw.rcItem.right - draw.rcItem.left) * 96 / dialog.dpi as i32;
            let height = (draw.rcItem.bottom - draw.rcItem.top) * 96 / dialog.dpi as i32;
            let id = draw.CtlID as usize;
            let disabled = draw.itemState & ODS_DISABLED != 0;
            let selected = dialog.selected(id) || id == APPLY;
            canvas.round(
                R::new(0, 0, width, height),
                8,
                if selected && !disabled {
                    paint::RED
                } else {
                    paint::FIELD
                },
                Some(if draw.itemState & ODS_FOCUS != 0 {
                    paint::MUTED
                } else {
                    paint::BORDER
                }),
            );
            let text = if id == TOGGLE {
                if dialog.config.enabled {
                    "开启".into()
                } else {
                    "关闭".into()
                }
            } else {
                let n = GetWindowTextLengthW(draw.hwndItem).max(0) as usize;
                let mut buffer = vec![0; n + 1];
                let n = GetWindowTextW(draw.hwndItem, buffer.as_mut_ptr(), buffer.len() as i32);
                String::from_utf16_lossy(&buffer[..n.max(0) as usize])
            };
            canvas.text(
                &text,
                R::new(4, 0, width - 8, height),
                13,
                500,
                if disabled { paint::DIM } else { paint::TEXT },
                DT_CENTER,
            );
            1
        }
        WM_COMMAND => {
            if (w >> 16) & 0xffff == BN_CLICKED as usize {
                dialog.command(hwnd, w & 0xffff);
            }
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        WM_TIMER => {
            dialog.poll(hwnd);
            0
        }
        WM_CLOSE => {
            if dialog.pending.is_some() {
                dialog.status = "正在完成灯效操作，请稍候再关闭。".into();
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_KEYDOWN if w == 27 => {
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            0
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}
pub unsafe fn open(owner: HWND) {
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskG2Lighting");
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);
    let font = CreateFontW(
        -scale(14),
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
    let brush = CreateSolidBrush(0x00261e1e);
    let mut dialog = Box::new(Dialog {
        config: LightingConfig::default(),
        actual_enabled: None,
        saved: false,
        buttons: vec![],
        font,
        brush,
        dpi,
        status: "正在准备灯光控制…".into(),
        error: false,
        writing: false,
        pending: None,
        custom_colors: [0; 16],
    });
    let mut owner_rect: RECT = zeroed();
    GetWindowRect(owner, &mut owner_rect);
    let mut bounds = RECT {
        left: 0,
        top: 0,
        right: scale(880),
        bottom: scale(580),
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    AdjustWindowRectEx(&mut bounds, style, 0, WS_EX_DLGMODALFRAME);
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class.as_ptr(),
        wide("G2 Pro 灯光 · Peripheral Desk").as_ptr(),
        style,
        owner_rect.left + ((owner_rect.right - owner_rect.left - width) / 2).max(0),
        owner_rect.top + ((owner_rect.bottom - owner_rect.top - height) / 2).max(0),
        width,
        height,
        owner,
        null_mut(),
        instance,
        (&mut *dialog as *mut Dialog).cast(),
    );
    if hwnd.is_null() {
        DeleteObject(font);
        DeleteObject(brush);
        return;
    }
    let dark = 1i32;
    DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    for (id, text, r) in [
        (TOGGLE, "开启", R::new(724, 126, 100, 36)),
        (STATIC, "常亮", R::new(344, 231, 145, 35)),
        (BREATHING, "呼吸", R::new(501, 231, 145, 35)),
        (RAINBOW, "彩虹", R::new(658, 231, 166, 35)),
        (COLOR, "选择 RGB", R::new(680, 279, 144, 36)),
        (SLOWER, "−", R::new(548, 369, 42, 36)),
        (FASTER, "+", R::new(710, 369, 42, 36)),
        (REFRESH, "重新读取", R::new(484, 472, 110, 36)),
        (CLOSE, "关闭", R::new(606, 472, 94, 36)),
        (APPLY, "应用灯效", R::new(712, 472, 140, 36)),
    ] {
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            scale(r.x),
            scale(r.y),
            scale(r.w),
            scale(r.h),
            hwnd,
            id as HMENU,
            instance,
            null(),
        );
        SendMessageW(button, WM_SETFONT, font as usize, 1);
        dialog.buttons.push((id, button));
    }
    EnableWindow(owner, 0);
    SetTimer(hwnd, 1, 80, None);
    dialog.begin(hwnd, false);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    let mut message: MSG = zeroed();
    while IsWindow(hwnd) != 0 {
        let result = GetMessageW(&mut message, null_mut(), 0, 0);
        if result <= 0 {
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
    EnableWindow(owner, 1);
    SetActiveWindow(owner);
    DeleteObject(dialog.font);
    DeleteObject(dialog.brush);
}
