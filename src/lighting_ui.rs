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
#[derive(Clone, Copy)]
enum Operation {
    Read,
    Apply,
    Toggle(bool),
}
struct Dialog {
    config: LightingConfig,
    actual_enabled: Option<bool>,
    saved: bool,
    dirty: bool,
    quick_switch: bool,
    buttons: Vec<(usize, HWND)>,
    font: HFONT,
    brush: HBRUSH,
    dpi: u32,
    status: String,
    error: bool,
    operation: Operation,
    pending: Option<Receiver<Result<LightingState, String>>>,
    custom_colors: [u32; 16],
}
impl Dialog {
    fn available(&self, id: usize) -> bool {
        self.pending.is_none()
            && match id {
                APPLY => self.actual_enabled.is_some(),
                // Legacy drivers edit a draft; newer drivers also recover an unknown switch.
                TOGGLE => true,
                SLOWER => self.config.mode != LightingMode::Static && self.config.speed > 1,
                FASTER => self.config.mode != LightingMode::Static && self.config.speed < 10,
                _ => true,
            }
    }
    unsafe fn update_buttons(&self) {
        for (id, button) in &self.buttons {
            EnableWindow(*button, i32::from(self.available(*id)));
        }
    }
    unsafe fn begin(&mut self, hwnd: HWND, operation: Operation) {
        if self.pending.is_some() {
            return;
        }
        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.operation = operation;
        self.error = false;
        self.status = match operation {
            Operation::Apply if config.enabled => "正在提交灯效参数，并读取设备返回的开关值…",
            Operation::Apply => "正在提交关闭命令；本次不会发送模式、颜色或速度…",
            Operation::Toggle(true) => "正在开启灯光并读回；保留模式和颜色，不发送灯效草稿…",
            Operation::Toggle(false) => "正在关闭灯光并读回；保留模式和颜色，不发送灯效草稿…",
            Operation::Read => "正在读取设备灯光开关与本工具保存配置…",
        }
        .into();
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        thread::spawn(move || {
            let _ = tx.send(match operation {
                Operation::Apply => audio_lighting::apply(&config),
                Operation::Toggle(enabled) => audio_lighting::set_enabled(enabled),
                Operation::Read => audio_lighting::read(),
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
        self.complete(result);
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    fn complete(&mut self, result: Result<LightingState, String>) {
        self.pending = None;
        match result {
            Ok(state) => {
                self.actual_enabled = Some(state.enabled);
                self.saved = state.saved.is_some();
                self.error = false;
                if let Operation::Toggle(expected) = self.operation {
                    // Toggle never loads saved/default mode values over a draft.
                    if !self.dirty {
                        self.config.enabled = state.enabled;
                    }
                    self.error = state.enabled != expected;
                    self.status = if self.error {
                        "开关返回值与请求不一致，未确认切换；灯效草稿已保留。".into()
                    } else if self.dirty {
                        "开关已切换并读回；未提交的模式、颜色和速度草稿已保留。".into()
                    } else {
                        "开关已切换并读回，未修改模式、颜色或速度。".into()
                    };
                    if !state.notice.trim().is_empty() {
                        self.status.push(' ');
                        self.status.push_str(&state.notice);
                    }
                } else if matches!(self.operation, Operation::Apply) {
                    // A register reply does not establish the visible effect.
                    // Keep the user's draft if the driver did not save it exactly.
                    self.dirty = state.enabled != self.config.enabled
                        || state.saved.as_ref() != Some(&self.config);
                    self.error = state.enabled != self.config.enabled;
                    self.status = if self.error {
                        "设备返回的开关值与草稿不一致，未确认应用；保留草稿供重试。".into()
                    } else if self.config.enabled {
                        "已提交参数；仅读回开关值，未确认模式、颜色和速度。请观察耳罩实际灯效。"
                            .into()
                    } else {
                        "已提交关闭；设备返回关闭值，实际是否熄灭请观察耳罩。其他灯效参数未发送。"
                            .into()
                    };
                    if !state.notice.trim().is_empty() {
                        self.status.push(' ');
                        self.status.push_str(&state.notice);
                    }
                } else {
                    self.config = state.saved.unwrap_or_default();
                    self.config.enabled = state.enabled;
                    self.dirty = false;
                    self.status = state.notice;
                }
            }
            Err(e) => {
                self.actual_enabled = None;
                self.status = e;
                self.error = true;
            }
        }
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
                    self.begin(hwnd, Operation::Apply);
                }
                return;
            }
            REFRESH => {
                self.begin(hwnd, Operation::Read);
                return;
            }
            TOGGLE => {
                if self.quick_switch {
                    self.begin(hwnd, Operation::Toggle(self.actual_enabled != Some(true)));
                } else {
                    self.config.enabled = !self.config.enabled;
                    self.dirty = true;
                    self.error = false;
                    self.status = if self.config.enabled {
                        "开灯草稿尚未发送；点击“应用灯效”生效。更新 G2 Pro 插件可使用即时开关。"
                    } else {
                        "关灯草稿尚未发送；点击“应用灯效”生效。更新 G2 Pro 插件可使用即时开关。"
                    }
                    .into();
                    self.update_buttons();
                    RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
                }
                return;
            }
            STATIC | BREATHING | RAINBOW => {
                self.config.mode = match id {
                    BREATHING => LightingMode::Breathing,
                    RAINBOW => LightingMode::Rainbow,
                    _ => LightingMode::Static,
                };
                self.config.enabled = true;
            }
            SLOWER | FASTER => {
                if self.config.mode == LightingMode::Static {
                    return;
                }
                self.config.speed = if id == SLOWER {
                    self.config.speed.saturating_sub(1).max(1)
                } else {
                    self.config.speed.saturating_add(1).min(10)
                };
                self.config.enabled = true;
            }
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
                if self.config.mode == LightingMode::Rainbow {
                    self.config.mode = LightingMode::Static;
                }
                self.config.enabled = true;
            }
            _ => return,
        }
        self.error = false;
        self.dirty = true;
        self.status = if self.config.enabled {
            if id == COLOR {
                "单色草稿准备开启灯光，尚未发送；点击“应用灯效”提交。"
            } else {
                "草稿准备开启灯光，尚未发送；点击“应用灯效”提交。"
            }
        } else {
            "草稿准备关闭灯光，尚未发送；点击“应用灯效”只提交关闭命令。"
        }
        .into();
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    fn selected(&self, id: usize) -> bool {
        match id {
            TOGGLE => {
                if self.quick_switch {
                    self.actual_enabled == Some(true)
                } else {
                    self.config.enabled
                }
            }
            STATIC => self.config.enabled && self.config.mode == LightingMode::Static,
            BREATHING => self.config.enabled && self.config.mode == LightingMode::Breathing,
            RAINBOW => self.config.enabled && self.config.mode == LightingMode::Rainbow,
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
            if self.quick_switch {
                "开关点击即提交；模式、颜色和速度需点击应用"
            } else {
                "开关与灯效先编辑草稿，点击应用生效；更新插件可即时切换"
            },
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
            if self.quick_switch {
                "灯光开关 · 即时切换"
            } else {
                "灯光开关 · 草稿"
            },
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
                "选择单色将切换为常亮，并准备开启；点击应用才发送。"
            } else {
                "选模式或颜色会准备开启，点击“应用灯效”才发送。"
            },
            R::new(344, 326, 480, 24),
            11,
            400,
            paint::DIM,
            DT_LEFT,
        );
        canvas.text(
            if self.config.mode == LightingMode::Static {
                "速度（常亮不适用）"
            } else {
                "速度"
            },
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
            if self.config.mode == LightingMode::Static {
                paint::DIM
            } else {
                paint::TEXT
            },
            DT_CENTER,
        );
        let actual = match self.actual_enabled {
            Some(true) => "上次设备返回：开关为开启",
            Some(false) => "上次设备返回：开关为关闭",
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
            if self.actual_enabled.is_none() {
                if self.quick_switch {
                    "未确认开关；可重新读取（放弃草稿），或“尝试开灯”（保留草稿）。"
                } else {
                    "未确认开关；重新读取将放弃草稿。更新 G2 Pro 插件后可使用即时开关。"
                }
            } else if self.dirty && self.config.enabled && self.actual_enabled == Some(false) {
                "设备当前关闭；灯效草稿尚未提交，点击“应用并开灯”将开启并发送草稿。"
            } else if self.dirty {
                "当前为未应用草稿；模式、颜色和速度不能完整读回，请观察实际灯效。"
            } else if self.saved {
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
                if !dialog.quick_switch {
                    if dialog.config.enabled {
                        "草稿：开启".into()
                    } else {
                        "草稿：关闭".into()
                    }
                } else {
                    match dialog.actual_enabled {
                        Some(true) => "已开启".into(),
                        Some(false) => "已关闭".into(),
                        None => "尝试开灯".into(),
                    }
                }
            } else if id == APPLY && dialog.config.enabled && dialog.actual_enabled == Some(false) {
                "应用并开灯".into()
            } else if id == REFRESH && dialog.dirty {
                "放弃并重读".into()
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
        dirty: false,
        quick_switch: crate::plugin_manager::installed("hecate-g2-pro")
            .ok()
            .flatten()
            .is_some_and(|driver| {
                driver
                    .manifest
                    .capabilities
                    .iter()
                    .any(|c| c == "lighting-switch")
            }),
        buttons: vec![],
        font,
        brush,
        dpi,
        status: "正在准备灯光控制…".into(),
        error: false,
        operation: Operation::Read,
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
    dialog.begin(hwnd, Operation::Read);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> Dialog {
        Dialog {
            config: LightingConfig::default(),
            actual_enabled: Some(false),
            saved: false,
            dirty: false,
            quick_switch: true,
            buttons: vec![],
            font: null_mut(),
            brush: null_mut(),
            dpi: 96,
            status: String::new(),
            error: false,
            operation: Operation::Read,
            pending: None,
            custom_colors: [0; 16],
        }
    }

    #[test]
    fn hardware_off_and_failed_read_leave_recovery_available() {
        let mut dialog = dialog();
        assert!(dialog.available(APPLY), "hardware off must allow Apply");
        dialog.quick_switch = false;
        assert!(
            dialog.available(TOGGLE),
            "legacy plugins retain draft toggle"
        );
        assert!(
            dialog.selected(TOGGLE),
            "legacy selection follows its on draft"
        );
        dialog.config.enabled = false;
        assert!(!dialog.selected(TOGGLE));
        assert!(dialog.available(APPLY), "legacy plugins retain Apply");
        dialog.quick_switch = true;

        let (_tx, rx) = mpsc::channel();
        dialog.pending = Some(rx);
        for id in [APPLY, TOGGLE, REFRESH, CLOSE] {
            assert!(!dialog.available(id), "pending operation must serialize UI");
        }
        dialog.complete(Err("模拟读取失败".into()));
        assert!(dialog.pending.is_none());
        assert!(dialog.error);
        assert_eq!(dialog.actual_enabled, None, "failure is not confirmed off");
        assert!(!dialog.available(APPLY));
        assert!(dialog.available(REFRESH));
        assert!(
            dialog.available(TOGGLE),
            "new driver can attempt recovery on"
        );
        assert!(!dialog.selected(TOGGLE));
    }

    #[test]
    fn immediate_toggle_preserves_unsent_draft_without_saved_config() {
        let mut dialog = dialog();
        let draft = LightingConfig {
            enabled: true,
            mode: LightingMode::Breathing,
            rgb: [123, 45, 67],
            speed: 7,
        };
        dialog.config = draft.clone();
        dialog.dirty = true;
        dialog.operation = Operation::Toggle(false);
        dialog.complete(Ok(LightingState {
            enabled: false,
            saved: None,
            notice: "首次读取，没有本工具保存的模式".into(),
        }));
        assert_eq!(dialog.config, draft);
        assert!(dialog.dirty);
        assert!(!dialog.error);
        assert_eq!(dialog.actual_enabled, Some(false));
        assert!(dialog.available(APPLY));
        assert!(!dialog.selected(TOGGLE));

        dialog.operation = Operation::Toggle(true);
        dialog.complete(Ok(LightingState {
            enabled: false,
            saved: None,
            notice: String::new(),
        }));
        assert!(
            dialog.error,
            "a mismatched switch reply cannot report success"
        );
        assert_eq!(dialog.actual_enabled, Some(false));
        assert_eq!(dialog.config, draft);
    }
}
