//! Per-device lighting editor for accessory lighting plugins.
//!
//! Unlike the legacy G2 editor, every displayed value here comes from a
//! successful device readback. Plugin metadata supplies the supported modes
//! and level bounds; the UI never guesses a device's capabilities.
use crate::{
    hid::wide,
    paint::{self, Canvas, R},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
const COLOR: usize = 102;
const SPEED_DOWN: usize = 103;
const SPEED_UP: usize = 104;
const BRIGHTNESS_DOWN: usize = 105;
const BRIGHTNESS_UP: usize = 106;
const APPLY: usize = 107;
const REFRESH: usize = 108;
const CLOSE: usize = 109;
const MODE_BASE: usize = 200;
const TIMER: usize = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LightingConfig {
    enabled: bool,
    effect: u8,
    rgb: [u8; 3],
    speed: u8,
    brightness: u8,
    music_sync: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct LightingMode {
    id: u8,
    label: String,
    color: bool,
    music_sync: bool,
}

#[derive(Clone)]
struct Capabilities {
    modes: Vec<LightingMode>,
    speed_max: u8,
    brightness_max: u8,
}

#[derive(Debug, Deserialize)]
struct LightingState {
    enabled: bool,
    saved: Option<LightingConfig>,
    notice: String,
    readback: bool,
}

#[derive(Clone, Copy)]
enum Operation {
    Read,
    Apply,
    Toggle(bool),
}

struct Dialog {
    key: String,
    name: String,
    capabilities: Capabilities,
    supports_switch: bool,
    config: Option<LightingConfig>,
    confirmed: Option<LightingConfig>,
    actual_enabled: Option<bool>,
    dirty: bool,
    stale: bool,
    mode_buttons: Vec<(usize, HWND)>,
    buttons: Vec<(usize, HWND)>,
    font: HFONT,
    dpi: u32,
    status: String,
    error: bool,
    operation: Operation,
    pending: Option<Receiver<Result<LightingState, String>>>,
    custom_colors: [u32; 16],
}

fn mode_meta(device: &Value) -> Result<Capabilities, String> {
    let modes: Vec<LightingMode> = serde_json::from_value(
        device
            .get("lighting_modes")
            .cloned()
            .ok_or("此驱动没有灯效模式能力说明")?,
    )
    .map_err(|e| format!("灯效模式说明无效：{e}"))?;
    let level = |key: &str| -> Result<u8, String> {
        device
            .get(key)
            .and_then(Value::as_u64)
            .filter(|v| *v <= u8::MAX as u64)
            .map(|v| v as u8)
            .ok_or_else(|| format!("驱动没有有效的 {key} 灯效范围"))
    };
    if modes.is_empty()
        || modes.len() > 6
        || modes.iter().any(|m| m.label.is_empty())
        || modes
            .iter()
            .map(|m| m.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != modes.len()
    {
        return Err("驱动灯效模式表为空、重复或过长".into());
    }
    Ok(Capabilities {
        modes,
        speed_max: level("lighting_speed_max")?,
        brightness_max: level("lighting_brightness_max")?,
    })
}

fn valid_config(config: &LightingConfig, capabilities: &Capabilities) -> bool {
    capabilities
        .modes
        .iter()
        .any(|mode| mode.id == config.effect && mode.music_sync == config.music_sync)
        && config.speed <= capabilities.speed_max
        && config.brightness <= capabilities.brightness_max
}

fn can_apply(
    actual_enabled: Option<bool>,
    confirmed: Option<&LightingConfig>,
    draft: Option<&LightingConfig>,
    capabilities: &Capabilities,
) -> bool {
    actual_enabled.is_some()
        && confirmed.is_some_and(|config| valid_config(config, capabilities))
        && draft.is_some_and(|config| valid_config(config, capabilities))
}

fn draft_after_toggle(
    draft: &Option<LightingConfig>,
    saved: Option<LightingConfig>,
    dirty: bool,
) -> Option<LightingConfig> {
    if dirty {
        draft.clone()
    } else {
        saved
    }
}

fn select_mode(config: &mut LightingConfig, baseline: &LightingConfig, mode: &LightingMode) {
    config.effect = mode.id;
    config.music_sync = mode.music_sync;
    config.enabled = true;
    if !mode.color {
        config.rgb = baseline.rgb;
    }
}

fn invoke(
    key: &str,
    operation: Operation,
    config: Option<&LightingConfig>,
) -> Result<LightingState, String> {
    let request = match operation {
        Operation::Read => json!({"op":"lighting-read"}),
        Operation::Apply => json!({"op":"lighting-apply","config":config.ok_or("缺少灯效草稿")?}),
        Operation::Toggle(enabled) => json!({"op":"lighting-switch","enabled":enabled}),
    };
    let value = crate::plugin_manager::invoke(key, request)?;
    let state: LightingState =
        serde_json::from_value(value).map_err(|e| format!("驱动返回的灯效状态无效：{e}"))?;
    if !state.readback {
        return Err("驱动未确认灯效来自设备读回；本次状态不作为已确认结果".into());
    }
    Ok(state)
}

impl Dialog {
    fn mode(&self, effect: u8) -> Option<&LightingMode> {
        self.capabilities
            .modes
            .iter()
            .find(|mode| mode.id == effect)
    }

    fn available(&self, id: usize) -> bool {
        if self.pending.is_some() {
            return false;
        }
        match id {
            APPLY => can_apply(
                self.actual_enabled,
                self.confirmed.as_ref(),
                self.config.as_ref(),
                &self.capabilities,
            ),
            TOGGLE => self.supports_switch || self.config.is_some(),
            REFRESH | CLOSE => true,
            COLOR => {
                self.confirmed.is_some()
                    && self
                        .config
                        .as_ref()
                        .and_then(|c| self.mode(c.effect))
                        .is_some_and(|m| m.color)
            }
            SPEED_DOWN => {
                self.confirmed.is_some() && self.config.as_ref().is_some_and(|c| c.speed > 0)
            }
            SPEED_UP => self
                .config
                .as_ref()
                .is_some_and(|c| self.confirmed.is_some() && c.speed < self.capabilities.speed_max),
            BRIGHTNESS_DOWN => {
                self.confirmed.is_some() && self.config.as_ref().is_some_and(|c| c.brightness > 0)
            }
            BRIGHTNESS_UP => self.config.as_ref().is_some_and(|c| {
                self.confirmed.is_some() && c.brightness < self.capabilities.brightness_max
            }),
            id if (MODE_BASE..MODE_BASE + 256).contains(&id) => {
                self.config.is_some() && self.confirmed.is_some()
            }
            _ => true,
        }
    }

    unsafe fn update_buttons(&self) {
        for (id, button) in self.buttons.iter().chain(self.mode_buttons.iter()) {
            EnableWindow(*button, i32::from(self.available(*id)));
        }
    }

    unsafe fn begin(&mut self, hwnd: HWND, operation: Operation) {
        if self.pending.is_some() {
            return;
        }
        let config = self.config.clone();
        if matches!(operation, Operation::Apply) && config.is_none() {
            return;
        }
        let key = self.key.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.operation = operation;
        self.error = false;
        self.status = match operation {
            Operation::Read => "正在读取设备灯效…",
            Operation::Apply => "正在写入完整灯效参数并读取设备返回值…",
            Operation::Toggle(true) => "正在开启灯效并读取设备返回值…",
            Operation::Toggle(false) => "正在关闭灯效并读取设备返回值…",
        }
        .into();
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        thread::spawn(move || {
            let _ = tx.send(invoke(&key, operation, config.as_ref()));
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
                let saved = state
                    .saved
                    .filter(|config| valid_config(config, &self.capabilities));
                self.actual_enabled = Some(state.enabled);
                self.confirmed = saved.clone();
                self.error = false;
                self.stale = false;
                match self.operation {
                    Operation::Read => {
                        self.config = saved.clone();
                        self.dirty = false;
                        self.status = if saved.is_some() {
                            "设备灯效已读回；显示的是设备当前值。".into()
                        } else {
                            "已读回设备开关，但驱动没有返回完整且受支持的灯效配置；编辑与应用保持关闭。".into()
                        };
                    }
                    Operation::Toggle(expected) => {
                        self.config = draft_after_toggle(&self.config, saved.clone(), self.dirty);
                        self.error = state.enabled != expected;
                        self.status = if self.error {
                            "设备返回的开关值与请求不一致；灯效草稿已保留。".into()
                        } else if self.dirty {
                            "开关已读回确认；未提交的灯效草稿已保留。".into()
                        } else {
                            "开关已读回确认；灯效参数未修改。".into()
                        };
                    }
                    Operation::Apply => {
                        let requested = self.config.as_ref();
                        let exact = saved.as_ref() == requested
                            && state.enabled == requested.is_some_and(|c| c.enabled);
                        self.error = !exact;
                        self.dirty = !exact;
                        if exact {
                            self.config = saved.clone();
                            self.status = "灯效写入后已从设备读回核对。".into();
                        } else {
                            self.status =
                                "设备读回值与灯效草稿不一致；草稿已保留，尚未确认应用。".into();
                        }
                    }
                }
                if !state.notice.trim().is_empty() {
                    self.status.push(' ');
                    self.status.push_str(&state.notice);
                }
            }
            Err(error) => {
                // Failed writes and switches leave both last confirmed values
                // and the user's in-progress draft intact.
                self.status = error;
                self.error = true;
                self.stale = true;
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
            REFRESH => {
                self.begin(hwnd, Operation::Read);
                return;
            }
            APPLY | 1 => {
                if self.available(APPLY) {
                    self.begin(hwnd, Operation::Apply);
                }
                return;
            }
            TOGGLE => {
                if self.supports_switch {
                    self.begin(hwnd, Operation::Toggle(self.actual_enabled != Some(true)));
                } else if let Some(config) = self.config.as_mut() {
                    config.enabled = !config.enabled;
                    self.dirty = true;
                    self.status = "开关草稿尚未发送；点击“应用灯效”提交。".into();
                    self.error = false;
                    self.update_buttons();
                }
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
                return;
            }
            COLOR => {
                let Some(config) = self.config.as_ref() else {
                    return;
                };
                let [r, g, b] = config.rgb;
                let mut choose: CHOOSECOLORW = zeroed();
                choose.lStructSize = size_of::<CHOOSECOLORW>() as u32;
                choose.hwndOwner = hwnd;
                choose.rgbResult = r as u32 | (g as u32) << 8 | (b as u32) << 16;
                choose.lpCustColors = self.custom_colors.as_mut_ptr();
                choose.Flags = CC_RGBINIT | CC_FULLOPEN;
                if ChooseColorW(&mut choose) != 0 {
                    if let Some(config) = self.config.as_mut() {
                        config.rgb = [
                            choose.rgbResult as u8,
                            (choose.rgbResult >> 8) as u8,
                            (choose.rgbResult >> 16) as u8,
                        ];
                        config.enabled = true;
                        self.dirty = true;
                        self.error = false;
                        self.status = "颜色草稿已更改；点击“应用灯效”提交。".into();
                    }
                } else {
                    return;
                }
            }
            SPEED_DOWN | SPEED_UP => {
                if let Some(config) = self.config.as_mut() {
                    config.speed = if id == SPEED_DOWN {
                        config.speed.saturating_sub(1)
                    } else {
                        config
                            .speed
                            .saturating_add(1)
                            .min(self.capabilities.speed_max)
                    };
                    config.enabled = true;
                    self.mark_draft("速度草稿已更改；点击“应用灯效”提交。");
                }
            }
            BRIGHTNESS_DOWN | BRIGHTNESS_UP => {
                if let Some(config) = self.config.as_mut() {
                    config.brightness = if id == BRIGHTNESS_DOWN {
                        config.brightness.saturating_sub(1)
                    } else {
                        config
                            .brightness
                            .saturating_add(1)
                            .min(self.capabilities.brightness_max)
                    };
                    config.enabled = true;
                    self.mark_draft("亮度草稿已更改；点击“应用灯效”提交。");
                }
            }
            id if (MODE_BASE..MODE_BASE + 256).contains(&id) => {
                let effect = (id - MODE_BASE) as u8;
                let mode = self.mode(effect).cloned();
                if let Some(mode) = mode {
                    if let (Some(config), Some(baseline)) =
                        (self.config.as_mut(), self.confirmed.as_ref())
                    {
                        select_mode(config, baseline, &mode);
                        self.mark_draft("模式草稿已更改；点击“应用灯效”提交。");
                    }
                }
            }
            _ => return,
        }
        self.update_buttons();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }

    fn mark_draft(&mut self, text: &str) {
        self.dirty = true;
        self.error = false;
        self.status = text.into();
    }

    fn selected(&self, id: usize) -> bool {
        if id == TOGGLE {
            return self.actual_enabled == Some(true);
        }
        if (MODE_BASE..MODE_BASE + 256).contains(&id) {
            return self
                .config
                .as_ref()
                .is_some_and(|config| config.effect as usize == id - MODE_BASE);
        }
        id == APPLY
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let canvas = Canvas::new(dc, self.dpi);
        canvas.rect(R::new(0, 0, 900, 660), paint::BG);
        canvas.gradient(R::new(0, 0, 900, 3), paint::RED, paint::ORANGE);
        canvas.text(
            &self.name,
            R::new(28, 20, 640, 36),
            25,
            600,
            paint::TEXT,
            DT_LEFT,
        );
        canvas.text(
            "设备当前值来自读回；更改会先保留为草稿",
            R::new(28, 62, 720, 22),
            13,
            400,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.round(
            R::new(28, 102, 238, 394),
            16,
            paint::PANEL,
            Some(paint::BORDER),
        );
        let config = self.config.as_ref();
        let color = config.map(|c| c.rgb).unwrap_or([100, 100, 100]);
        let rgb = 0xff000000 | (color[0] as u32) << 16 | (color[1] as u32) << 8 | color[2] as u32;
        canvas.ellipse(R::new(75, 147, 144, 144), paint::BG);
        canvas.arc(
            R::new(87, 159, 120, 120),
            0.,
            359.9,
            if self.actual_enabled == Some(false) {
                paint::BORDER
            } else {
                rgb
            },
            10.,
        );
        canvas.text(
            "灯效示意",
            R::new(48, 320, 198, 24),
            15,
            600,
            paint::TEXT,
            DT_CENTER,
        );
        canvas.text(
            if config.is_some() {
                "颜色仅供界面预览"
            } else {
                "尚未读到完整灯效配置"
            },
            R::new(39, 351, 216, 22),
            12,
            400,
            paint::DIM,
            DT_CENTER,
        );
        if let Some(config) = config {
            let mode = self.mode(config.effect);
            canvas.text(
                mode.map(|m| m.label.as_str()).unwrap_or("未知模式"),
                R::new(46, 403, 202, 26),
                15,
                500,
                paint::TEXT,
                DT_CENTER,
            );
        }

        canvas.round(
            R::new(286, 102, 586, 394),
            16,
            paint::PANEL,
            Some(paint::BORDER),
        );
        canvas.text(
            "灯效开关",
            R::new(310, 122, 200, 24),
            15,
            600,
            paint::TEXT,
            DT_LEFT,
        );
        let enabled_text = match self.actual_enabled {
            Some(true) => "当前开启",
            Some(false) => "当前关闭",
            None => "尚未读取",
        };
        canvas.text(
            enabled_text,
            R::new(500, 122, 148, 24),
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
            "模式",
            R::new(310, 174, 120, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            "RGB 颜色",
            R::new(310, 328, 140, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        let mode_color = config
            .and_then(|c| self.mode(c.effect))
            .is_some_and(|m| m.color);
        if mode_color {
            canvas.round(R::new(430, 321, 42, 36), 8, rgb, Some(paint::BORDER));
            canvas.text(
                &format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]),
                R::new(483, 327, 132, 24),
                14,
                500,
                paint::TEXT,
                DT_LEFT,
            );
        } else {
            canvas.text(
                "此模式不使用可选颜色",
                R::new(430, 327, 245, 24),
                13,
                400,
                paint::DIM,
                DT_LEFT,
            );
        }
        let speed = config
            .map(|c| c.speed.to_string())
            .unwrap_or_else(|| "—".into());
        let brightness = config
            .map(|c| c.brightness.to_string())
            .unwrap_or_else(|| "—".into());
        canvas.text(
            "速度",
            R::new(310, 382, 80, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            &format!("{speed} / {}", self.capabilities.speed_max),
            R::new(410, 382, 98, 22),
            14,
            500,
            paint::TEXT,
            DT_CENTER,
        );
        canvas.text(
            "亮度",
            R::new(310, 438, 80, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            &format!("{brightness} / {}", self.capabilities.brightness_max),
            R::new(410, 438, 98, 22),
            14,
            500,
            paint::TEXT,
            DT_CENTER,
        );

        let actual = match self.actual_enabled {
            Some(true) => "设备开关：开启",
            Some(false) => "设备开关：关闭",
            None => "设备开关：尚未确认",
        };
        canvas.text(
            actual,
            R::new(28, 512, 280, 22),
            13,
            500,
            if self.actual_enabled.is_some() {
                paint::GREEN
            } else {
                paint::MUTED
            },
            DT_LEFT,
        );
        let config_note = if self.confirmed.is_none() {
            "未读到有效的设备灯效配置；模式、颜色、速度和亮度不会显示为已确认。"
        } else if self.stale {
            "显示的是上次成功读回的值；刚才的操作失败，当前设备状态需重新读取确认。"
        } else if self.dirty {
            "当前显示的更改尚未提交；失败时草稿会保留。"
        } else {
            "灯效参数已从设备读回，可编辑并应用。"
        };
        canvas.text(
            config_note,
            R::new(28, 539, 840, 20),
            12,
            400,
            paint::DIM,
            DT_LEFT,
        );
        canvas.text(
            &self.status,
            R::new(28, 565, 840, 34),
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
            let background = paint::PANEL;
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
            canvas.round(
                R::new(0, 0, width, height),
                8,
                if dialog.selected(id) && !disabled {
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
            let n = GetWindowTextLengthW(draw.hwndItem).max(0) as usize;
            let mut buffer = vec![0; n + 1];
            let n = GetWindowTextW(draw.hwndItem, buffer.as_mut_ptr(), buffer.len() as i32);
            let text = if id == TOGGLE && dialog.supports_switch {
                match dialog.actual_enabled {
                    Some(true) => "关闭灯效".into(),
                    Some(false) => "开启灯效".into(),
                    None => "尝试开启".into(),
                }
            } else {
                String::from_utf16_lossy(&buffer[..n.max(0) as usize])
            };
            canvas.text(
                &text,
                R::new(4, 0, width - 8, height),
                12,
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
            KillTimer(hwnd, TIMER);
            0
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}

unsafe fn add_button(dialog: &mut Dialog, hwnd: HWND, id: usize, text: &str, rect: R) {
    let dpi = dialog.dpi;
    let font = dialog.font;
    let instance = GetModuleHandleW(null());
    let scale = |n: i32| n * dpi as i32 / 96;
    let button = CreateWindowExW(
        0,
        wide("BUTTON").as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
        scale(rect.x),
        scale(rect.y),
        scale(rect.w),
        scale(rect.h),
        hwnd,
        id as HMENU,
        instance,
        null(),
    );
    SendMessageW(button, WM_SETFONT, font as usize, 1);
    dialog.buttons.push((id, button));
}

fn show_error(owner: HWND, message: &str) {
    unsafe {
        MessageBoxW(
            owner,
            wide(message).as_ptr(),
            wide("灯效编辑器无法打开").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Open the generic editor for the device at the registry index used by the UI.
pub unsafe fn open(owner: HWND, model: usize) {
    let Some(spec) = crate::devices::REGISTRY.get(model) else {
        return;
    };
    let installed = match crate::plugin_manager::installed(spec.key) {
        Ok(Some(driver)) => driver,
        Ok(None) => {
            show_error(owner, "尚未安装此设备的灯效驱动。");
            return;
        }
        Err(error) => {
            show_error(owner, &error);
            return;
        }
    };
    let device = &installed.manifest.device;
    let capabilities = match mode_meta(device) {
        Ok(value) => value,
        Err(error) => {
            show_error(owner, &error);
            return;
        }
    };
    let supports_switch = installed
        .manifest
        .capabilities
        .iter()
        .any(|c| c == "lighting-switch");
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskGenericLighting");
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
    let mut dialog = Box::new(Dialog {
        key: spec.key.into(),
        name: spec.name.into(),
        capabilities,
        supports_switch,
        config: None,
        confirmed: None,
        actual_enabled: None,
        dirty: false,
        stale: false,
        mode_buttons: vec![],
        buttons: vec![],
        font,
        dpi,
        status: "正在准备读取设备灯效…".into(),
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
        right: scale(900),
        bottom: scale(660),
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    AdjustWindowRectEx(&mut bounds, style, 0, WS_EX_DLGMODALFRAME);
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let title = format!("{} 灯效 · Peripheral Desk", spec.name);
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class.as_ptr(),
        wide(&title).as_ptr(),
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
        return;
    }
    let dark = 1i32;
    DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    add_button(
        &mut dialog,
        hwnd,
        TOGGLE,
        if supports_switch {
            "尝试开关"
        } else {
            "开关草稿"
        },
        R::new(704, 115, 144, 34),
    );
    for (index, mode) in dialog.capabilities.modes.iter().enumerate() {
        let col = index % 3;
        let row = index / 3;
        let id = MODE_BASE + mode.id as usize;
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(&mode.label).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            scale(310 + col as i32 * 174),
            scale(202 + row as i32 * 44),
            scale(160),
            scale(34),
            hwnd,
            id as HMENU,
            instance,
            null(),
        );
        SendMessageW(button, WM_SETFONT, font as usize, 1);
        dialog.mode_buttons.push((id, button));
    }
    for (id, text, rect) in [
        (COLOR, "选择颜色", R::new(708, 321, 140, 36)),
        (SPEED_DOWN, "−", R::new(708, 377, 38, 32)),
        (SPEED_UP, "+", R::new(817, 377, 38, 32)),
        (BRIGHTNESS_DOWN, "−", R::new(708, 433, 38, 32)),
        (BRIGHTNESS_UP, "+", R::new(817, 433, 38, 32)),
        (REFRESH, "重新读取", R::new(505, 608, 104, 34)),
        (CLOSE, "关闭", R::new(621, 608, 94, 34)),
        (APPLY, "应用灯效", R::new(727, 608, 141, 34)),
    ] {
        add_button(&mut dialog, hwnd, id, text, rect);
    }
    EnableWindow(owner, 0);
    SetTimer(hwnd, TIMER, 80, None);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> Capabilities {
        Capabilities {
            modes: vec![
                LightingMode {
                    id: 0,
                    label: "常亮".into(),
                    color: true,
                    music_sync: false,
                },
                LightingMode {
                    id: 5,
                    label: "音乐".into(),
                    color: false,
                    music_sync: true,
                },
            ],
            speed_max: 4,
            brightness_max: 4,
        }
    }

    fn config() -> LightingConfig {
        LightingConfig {
            enabled: false,
            effect: 0,
            rgb: [12, 34, 56],
            speed: 2,
            brightness: 3,
            music_sync: false,
        }
    }

    #[test]
    fn hardware_off_still_allows_apply_from_a_confirmed_config() {
        let caps = capabilities();
        let saved = config();
        assert!(can_apply(Some(false), Some(&saved), Some(&saved), &caps));
    }

    #[test]
    fn successful_switch_keeps_a_dirty_lighting_draft() {
        let draft = Some(config());
        let mut saved = config();
        saved.rgb = [200, 100, 50];
        assert_eq!(draft_after_toggle(&draft, Some(saved), true), draft);
    }

    #[test]
    fn unconfirmed_or_unsupported_lighting_cannot_be_applied() {
        let caps = capabilities();
        let saved = config();
        assert!(!can_apply(None, Some(&saved), Some(&saved), &caps));
        let mut unsupported = saved.clone();
        unsupported.effect = 4;
        assert!(!can_apply(
            Some(true),
            Some(&saved),
            Some(&unsupported),
            &caps
        ));
        assert!(!can_apply(Some(true), None, Some(&saved), &caps));
    }

    #[test]
    fn switching_to_a_non_color_effect_uses_readback_rgb() {
        let baseline = config();
        let mut draft = baseline.clone();
        draft.rgb = [200, 100, 50];
        select_mode(
            &mut draft,
            &baseline,
            &LightingMode {
                id: 5,
                label: "音乐".into(),
                color: false,
                music_sync: true,
            },
        );
        assert_eq!(draft.effect, 5);
        assert!(draft.music_sync);
        assert_eq!(draft.rgb, baseline.rgb);
    }
}
