//! Page painting for the main window: home dashboard, device detail and preferences.
use super::widgets::{FONT_LG, FONT_MD, RADIUS_LG, RADIUS_MD};
use super::{widget_key, App, Command, Page, CARD, CHIP, ENTRY, GHOST, HOME_CONTENT_Y, SIDEBAR};
use crate::{
    backend::{self, Action},
    devices::{Kind, REGISTRY},
    model::Tuning,
    paint::{Canvas, R, *},
    system_battery,
};
use std::mem::zeroed;
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{Controls::*, WindowsAndMessaging::*},
};

/// Navigation/transition state owned by the main-window rework.
#[derive(Default)]
pub(crate) struct NavFx {
    started: Option<std::time::Instant>,
}

impl NavFx {
    pub(crate) fn start(&mut self) {
        self.started = Some(std::time::Instant::now());
    }

    fn value(&self) -> f32 {
        self.started
            .map(|start| (start.elapsed().as_millis() as f32 / 250.).clamp(0., 1.))
            .unwrap_or(1.)
    }

    pub(crate) fn tick(&mut self, _hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        self.value() < 1.
    }

    pub(crate) fn on_move(&mut self, _x: i32, _y: i32) {}

    pub(crate) fn on_leave(&mut self) {}
}

pub(super) fn system_reading(device: &system_battery::SystemBattery) -> String {
    if let Some(percent) = device.state.percent {
        format!("{percent}%")
    } else if let Some(level) = &device.level {
        format!("{level}（等级）")
    } else {
        "电量未知".into()
    }
}
pub(super) fn detail_y(y: i32) -> i32 {
    145 + (y - 78) * 85 / 100
}
pub(super) fn sleep_label(seconds: u16, full: bool) -> String {
    if seconds == 0 {
        "不休眠".into()
    } else if full {
        format!("{} 分钟", seconds / 60)
    } else {
        format!("{} 分", seconds / 60)
    }
}
pub(super) fn is_volume(key: Tuning) -> bool {
    matches!(
        key,
        Tuning::GameVolume | Tuning::ChatVolume | Tuning::MicVolume
    )
}
pub(super) fn format_number(v: u32) -> String {
    if v >= 1000 {
        format!("{},{:03}", v / 1000, v % 1000)
    } else {
        v.to_string()
    }
}
pub(super) fn connection(s: &backend::State, installed: bool) -> &str {
    if !installed {
        return "尚未安装插件";
    }
    if crate::plugin_manager::web_driver_owns(&s.key) {
        return "驱动中心控制中";
    }
    if !s.fresh {
        return if s.present {
            "接口在线 · 数据待更新"
        } else {
            "设备未连接"
        };
    }
    s.connection_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .unwrap_or(if s.wired {
            "USB 有线连接"
        } else {
            "已连接"
        })
}
pub(super) fn reading(s: &backend::State, spec: &crate::devices::DeviceSpec) -> String {
    if crate::plugin_manager::web_driver_owns(&s.key) && s.percent.is_none() {
        return "驱动中心控制中".into();
    }
    match (s.fresh, s.percent) {
        (true, Some(p)) => format!("{p}%"),
        (false, Some(p)) => format!("上次 {p}%"),
        (true, None) if spec.has_g2_audio() => "USB 供电".into(),
        (true, None) if spec.supports("battery") => "电量未知".into(),
        (true, None) => "已连接".into(),
        (false, None) if s.present => "待更新".into(),
        _ => "未连接".into(),
    }
}
fn stage_values(values: &[u16]) -> String {
    values
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join("  /  ")
}
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Mouse => "鼠标",
        Kind::Keyboard => "键盘",
        Kind::Headset => "耳机",
        Kind::Accessory => "其他外设",
    }
}
pub(super) fn connection_color(s: &backend::State) -> u32 {
    if s.fresh {
        GREEN
    } else if s.present {
        AMBER
    } else {
        DIM
    }
}
pub(super) fn power(s: &backend::State) -> &'static str {
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

impl App {
    pub(super) unsafe fn sidebar_overlay(&self, c: &Canvas) {
        c.gradient_v(R::new(0, 0, SIDEBAR, self.height), 0xff111117, 0xff0e0e12);
        c.line(SIDEBAR, 0, SIDEBAR, self.height, BORDER, 1.);
        c.text(
            "外设盒",
            R::new(20, 20, SIDEBAR - 40, 30),
            18,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "PERIPHERAL DESK",
            R::new(20, 46, SIDEBAR - 40, 18),
            10,
            500,
            DIM,
            DT_LEFT,
        );
        c.text(
            "设备",
            R::new(18, 252, SIDEBAR - 36, 20),
            11,
            600,
            DIM,
            DT_LEFT,
        );
    }

    pub(super) unsafe fn device_summary(&self, c: &Canvas, m: usize) {
        c.rect(R::new(SIDEBAR + 1, 66, 189, self.height - 66), BG);
        let s = &self.snapshot.devices[m];
        let spec = &REGISTRY[m];
        let panel = R::new(SIDEBAR + 16, 88, 158, (self.height - 172).max(400));
        c.soft_shadow(panel, 12);
        c.round(panel, RADIUS_MD, PANEL, Some(BORDER));
        if s.fresh {
            c.glow(panel.x + panel.w / 2, panel.y + 68, 56);
        }
        c.image(
            self.images.images[m],
            R::new(panel.x + 18, panel.y + 22, 122, 98),
            s.fresh,
        );
        c.text(
            spec.brand,
            R::new(panel.x + 12, panel.y + 132, 134, 20),
            10,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            spec.name,
            R::new(panel.x + 12, panel.y + 156, 134, 42),
            13,
            600,
            TEXT,
            DT_LEFT,
        );
        c.ellipse(
            R::new(panel.x + 13, panel.y + 210, 6, 6),
            connection_color(s),
        );
        c.text(
            connection(s, self.images.installed[m]),
            R::new(panel.x + 26, panel.y + 202, 120, 34),
            10,
            400,
            MUTED,
            DT_LEFT,
        );
        c.line(
            panel.x + 14,
            panel.y + 248,
            panel.x + panel.w - 14,
            panel.y + 248,
            BORDER,
            1.,
        );
        if spec.supports("battery") || s.percent.is_some() {
            c.ring(
                R::new(panel.x + 29, panel.y + 272, 100, 100),
                s.percent,
                s.fresh,
            );
            c.text(
                if !s.fresh && s.percent.is_some() {
                    "上次读数"
                } else {
                    power(s)
                },
                R::new(panel.x + 8, panel.y + 386, panel.w - 16, 24),
                11,
                400,
                MUTED,
                DT_CENTER,
            );
        } else {
            c.text(
                kind_name(spec.kind),
                R::new(panel.x + 12, panel.y + 266, panel.w - 24, 24),
                14,
                500,
                TEXT,
                DT_CENTER,
            );
            if spec.has_g2_audio() {
                c.text(
                    "USB 供电",
                    R::new(panel.x + 12, panel.y + 298, panel.w - 24, 24),
                    12,
                    400,
                    MUTED,
                    DT_CENTER,
                );
            }
        }
    }

    pub(super) fn eq_hit(&self, x: i32, y: i32) -> Option<(Tuning, R)> {
        if !self.can_adjust() || self.audio_page != 1 {
            return None;
        }
        let Page::Device(m) = self.page else {
            return None;
        };
        if !REGISTRY[m].has_g2_audio() {
            return None;
        }
        let bw = (self.width - 394 - 28 - 72) / 10;
        crate::audio_fx::EQ_BANDS
            .iter()
            .enumerate()
            .find_map(|(i, (key, _))| {
                let rect = R::new(
                    430 + i as i32 * bw,
                    detail_y(314),
                    bw - 10,
                    detail_y(504) - detail_y(314),
                );
                (rect.contains(x, y) && self.tuning_value(m, *key).is_some())
                    .then_some((*key, rect))
            })
    }
    pub(super) unsafe fn eq_slide(&mut self, hwnd: HWND, key: Tuning, rect: R, y: i32) {
        let v = ((1. - ((y - rect.y) as f32 / rect.h as f32).clamp(0., 1.)) * 40.).round() as u16;
        self.schedule(hwnd, Action::Tune(key, v));
    }
    unsafe fn eq_panel(&self, c: &Canvas, m: usize) {
        let bw = (self.width - 394 - 28 - 72) / 10;
        let top = detail_y(314);
        let bottom = detail_y(504);
        let middle = (top + bottom) / 2;
        for (i, (key, label)) in crate::audio_fx::EQ_BANDS.iter().enumerate() {
            let x = 430 + i as i32 * bw;
            let center = x + (bw - 10) / 2;
            let value = self.tuning_value(m, *key);
            let available = value.is_some() && self.snapshot.devices[m].fresh;
            c.line(center, top, center, bottom, BORDER, 5.);
            c.line(center - 7, middle, center + 7, middle, DIM, 1.);
            if let Some(v) = value {
                let y = bottom - (v as f32 / 40. * (bottom - top) as f32).round() as i32;
                c.line(
                    center,
                    middle,
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
                R::new(x - 5, detail_y(239), bw, 25),
                12,
                400,
                MUTED,
                DT_CENTER,
            );
            c.text(
                label,
                R::new(x - 5, detail_y(556) + 6, bw, 22),
                12,
                400,
                DIM,
                DT_CENTER,
            );
        }
    }
    pub(super) unsafe fn background(&self, c: &Canvas) {
        c.rect(R::new(0, 0, self.width, self.height), BG);
        c.gradient_v(R::new(0, 0, SIDEBAR, self.height), 0xff111117, 0xff0e0e12);
        c.line(SIDEBAR, 0, SIDEBAR, self.height, BORDER, 1.);
        c.gradient_v(
            R::new(SIDEBAR + 1, 0, self.width - SIDEBAR - 1, 66),
            crate::paint::blend(BG, 0xff25131a, self.fx.nav.value()),
            BG,
        );
        c.line(SIDEBAR + 24, 65, self.width - 28, 65, 0xff19191f, 1.);
        c.text(
            "外设盒",
            R::new(20, 20, SIDEBAR - 40, 30),
            18,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "PERIPHERAL DESK",
            R::new(20, 46, SIDEBAR - 40, 18),
            10,
            500,
            DIM,
            DT_LEFT,
        );
        c.text(
            "设备",
            R::new(18, 252, SIDEBAR - 36, 20),
            11,
            600,
            DIM,
            DT_LEFT,
        );
    }
    pub(super) unsafe fn home(&self, c: &Canvas) {
        c.text(
            "我的设备",
            R::new(SIDEBAR + 24, 88, 600, 34),
            24,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
                "尚未加载设备驱动，先从驱动商店添加你的设备"
            } else if REGISTRY.is_empty() {
                "选择设备，查看 Windows 提供的电量"
            } else {
                "选择设备，查看电量并调整设置"
            },
            R::new(SIDEBAR + 26, 124, 700, 23),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
            let center_y = HOME_CONTENT_Y + (self.height - HOME_CONTENT_Y - 86) / 2;
            c.round(
                R::new(
                    SIDEBAR + 24,
                    HOME_CONTENT_Y,
                    self.width - SIDEBAR - 48,
                    self.height - HOME_CONTENT_Y - 86,
                ),
                20,
                PANEL,
                Some(BORDER),
            );
            c.text(
                "从添加设备驱动开始",
                R::new(SIDEBAR + 64, center_y - 98, self.width - SIDEBAR - 128, 42),
                28,
                600,
                TEXT,
                DT_CENTER,
            );
            c.text(
                "驱动按设备下载，只安装你需要的型号。",
                R::new(SIDEBAR + 64, center_y - 44, self.width - SIDEBAR - 128, 26),
                15,
                400,
                MUTED,
                DT_CENTER,
            );
            c.text(
                "安装完成后，按提示重启即可使用。",
                R::new(SIDEBAR + 64, center_y + 76, self.width - SIDEBAR - 128, 26),
                13,
                400,
                DIM,
                DT_CENTER,
            );
        }
    }
    pub(super) unsafe fn system_detail(&self, c: &Canvas, m: usize) {
        let Some(device) = self.snapshot.system_devices.get(m) else {
            return;
        };
        let left = SIDEBAR + 24;
        let width = self.width - left - 48;
        c.text(
            "Windows 设备",
            R::new(left, 88, 600, 34),
            24,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "系统提供电量 · 只读状态",
            R::new(left + 2, 124, width - 4, 22),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        c.soft_shadow(R::new(left, 168, width, 360), 14);
        c.round(
            R::new(left, 168, width, 360),
            RADIUS_LG,
            PANEL,
            Some(BORDER),
        );
        c.text(
            &device.name,
            R::new(left + 32, 202, width - 64, 48),
            24,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "Windows 系统电量 · 只读",
            R::new(left + 34, 252, width - 68, 24),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            &system_reading(device),
            R::new(left + 34, 316, width - 68, 78),
            48,
            500,
            TEXT,
            DT_LEFT,
        );
        if device.state.charging == Some(true) {
            c.text(
                "正在充电",
                R::new(left + 34, 416, width - 68, 26),
                14,
                400,
                MUTED,
                DT_LEFT,
            );
        }
        c.text(
            "由 Windows 提供的只读电量数据",
            R::new(left + 34, 478, width - 68, 22),
            12,
            400,
            DIM,
            DT_LEFT,
        );
    }
    pub(super) unsafe fn detail(&self, c: &Canvas, m: usize) {
        let s = &self.snapshot.devices[m];
        let spec = &REGISTRY[m];
        let h = self.height;
        let rx = 394;
        let rw = self.width - rx - 28;
        let cx = rx + 30;
        c.text(spec.name, R::new(rx, 78, rw, 34), 23, 600, TEXT, DT_LEFT);
        c.text(
            spec.validation,
            R::new(rx + 1, 113, rw, 22),
            12,
            400,
            MUTED,
            DT_LEFT,
        );
        c.round(R::new(rx, 145, rw, h - 227), RADIUS_LG, PANEL, Some(BORDER));
        if self.show_evidence {
            c.text(
                "适配信息",
                R::new(cx, 175, rw - 60, 36),
                22,
                600,
                TEXT,
                DT_LEFT,
            );
            let links = &spec.evidence["connections"];
            let connections = match links {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" / "),
                _ => String::new(),
            };
            let firmware = spec.evidence["firmware"]
                .as_str()
                .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("unknown"))
                .unwrap_or("未记录");
            let report = spec.evidence["project_validation"]
                .as_str()
                .filter(|text| !text.is_empty())
                .unwrap_or(spec.validation);
            for (label, value, y, height) in [
                ("型号插件", spec.key, 235, 36),
                ("安装版本", &spec.plugin_version, 288, 36),
                (
                    "适用连接",
                    if connections.is_empty() {
                        "见插件说明"
                    } else {
                        &connections
                    },
                    342,
                    78,
                ),
                ("验证固件", firmware, 438, 42),
                ("验证记录", report, 508, (h - 610).max(92)),
            ] {
                c.line(cx, y - 12, rx + rw - 30, y - 12, BORDER, 1.);
                c.text(label, R::new(cx, y, 92, 26), 13, 400, MUTED, DT_LEFT);
                c.text(
                    value,
                    R::new(cx + 100, y + 4, rw - 160, height),
                    14,
                    500,
                    TEXT,
                    DT_LEFT | DT_WORDBREAK,
                );
            }
            return;
        }
        if self.hub_controls(m) {
            c.text(
                "正在使用迈从驱动中心",
                R::new(cx, 179, rw - 60, 38),
                22,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "在驱动中心选择设备并完成设置。关闭窗口后，设备页会恢复读取。",
                R::new(cx, 237, rw - 60, 72),
                14,
                400,
                MUTED,
                DT_LEFT | DT_WORDBREAK,
            );
            return;
        }
        if spec.has_g2_audio() {
            self.advanced_panel(c, m);
            return;
        }
        if spec.read_only() {
            c.text(
                "设备概览",
                R::new(cx, 175, rw - 60, 36),
                22,
                600,
                TEXT,
                DT_LEFT,
            );
            let mut rows = vec![("连接", connection(s, self.images.installed[m]).to_owned())];
            if spec.supports("battery") || s.percent.is_some() {
                rows.push(("电量", reading(s, spec)));
                if s.fresh && s.charging.is_some() {
                    rows.push(("充电状态", power(s).into()));
                }
            }
            if s.fresh {
                if let Some(dpi) = s.dpi {
                    rows.push(("当前 DPI", format_number(dpi.into())));
                }
                if let Some(stage) = s.active_stage {
                    rows.push(("当前档位", format!("第 {} 档", u16::from(stage) + 1)));
                }
                if let Some(rate) = s.rate {
                    rows.push(("配置回报率", format!("{rate} Hz")));
                }
            }
            if spec.switch_only() {
                rows.push((
                    "背光",
                    if s.fresh {
                        match s.light {
                            Some(0) => "关闭",
                            Some(1) => "开启",
                            _ => "未知",
                        }
                    } else {
                        "未知"
                    }
                    .into(),
                ));
            }
            if let Some((_, sample)) = self
                .pointer_sample
                .as_ref()
                .filter(|(model, _)| *model == m)
            {
                let state = &sample.state;
                if !state.dpi_stages.is_empty() {
                    rows.push(("DPI 档位（X）", stage_values(&state.dpi_stages)));
                }
                if let Some(y) = sample.dpi_stages_y.as_deref().filter(|v| !v.is_empty()) {
                    rows.push(("DPI 档位（Y）", stage_values(y)));
                }
                if let Some(rate) = state.rate {
                    rows.push(("配置回报率", format!("{rate} Hz")));
                } else if let Some([usb, wireless]) = sample.rate_indices {
                    rows.push(("回报率索引", format!("USB {usb} · 2.4G {wireless}")));
                }
                if let Some(stage) = state.active_stage {
                    rows.push(("设备上报档位", format!("第 {} 档", u16::from(stage) + 1)));
                }
                if !state.settings_error.is_empty() {
                    rows.push(("读取说明", state.settings_error.clone()));
                }
            }
            rows.push(("插件版本", spec.plugin_version.clone()));
            let spacing = ((h - 340) / rows.len() as i32).clamp(39, 52);
            for (i, (label, value)) in rows.iter().enumerate() {
                let y = 230 + i as i32 * spacing;
                c.line(cx, y - 6, rx + rw - 30, y - 6, BORDER, 1.);
                c.text(label, R::new(cx, y, 110, 36), 13, 400, MUTED, DT_LEFT);
                c.text(
                    value,
                    R::new(cx + 126, y, rw - 188, 36),
                    15,
                    500,
                    TEXT,
                    DT_LEFT,
                );
            }
            return;
        }
        if self.advanced && !spec.advanced.is_empty() {
            self.advanced_panel(c, m);
            return;
        }
        if spec.dpi_range.is_some() {
            c.text(
                "DPI 灵敏度",
                R::new(cx, detail_y(101), 300, 30),
                FONT_LG,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                &format!("点击快捷值或拖动滑块调整，滚轮可 {} 步进", spec.dpi_step),
                R::new(cx, detail_y(137), rw - 60, 24),
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
                R::new(rx + rw - 230, detail_y(97), 170, 52),
                38,
                300,
                if s.fresh { TEXT } else { DIM },
                DT_RIGHT,
            );
            c.text(
                "DPI",
                R::new(rx + rw - 53, detail_y(115), 30, 24),
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
                R::new(cx, detail_y(286), rw - 60, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
        } else if spec.key == "mchose-g75-pro" {
            c.text(
                "键盘灯效",
                R::new(cx, detail_y(101), 300, 32),
                FONT_LG,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "选择灯光效果，保留其他灯光及按键设置",
                R::new(cx, detail_y(137), rw - 60, 24),
                12,
                400,
                DIM,
                DT_LEFT,
            );
        }
        c.line(cx, detail_y(337), rx + rw - 30, detail_y(337), BORDER, 1.);
        if !spec.rates_for(s.wired).is_empty() {
            c.text(
                "回报率",
                R::new(cx, detail_y(354), 135, 29),
                15,
                500,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "决定输入上报频率",
                R::new(cx, detail_y(382), 135, 22),
                11,
                400,
                DIM,
                DT_LEFT,
            );
        }
        if !spec.sleeps.is_empty() {
            c.text(
                "自动休眠",
                R::new(cx, detail_y(421), 135, 29),
                15,
                500,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "空闲后进入省电状态",
                R::new(cx, detail_y(451), 135, 22),
                11,
                400,
                DIM,
                DT_LEFT,
            );
        }
        let lower_section = (h - 804).clamp(-22, 0);
        c.line(
            cx,
            detail_y(492) + lower_section,
            rx + rw - 30,
            detail_y(492) + lower_section,
            BORDER,
            1.,
        );
        c.text(
            "插件信息",
            R::new(cx, detail_y(513) + lower_section, 200, 23),
            15,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            &format!(
                "{}  ·  {}  ·  {}",
                spec.brand,
                kind_name(spec.kind),
                spec.plugin_version
            ),
            R::new(cx, detail_y(542) + lower_section, rw - 70, 22),
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
        let audio = REGISTRY[m].has_g2_audio();
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
            R::new(cx, detail_y(101), 350, 32),
            FONT_LG,
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
            R::new(cx, detail_y(138), rw - 60, 23),
            13,
            400,
            DIM,
            DT_LEFT,
        );
        if !audio && self.settings_pages(m) > 1 {
            c.text(
                &format!("{} / {}", self.settings_page + 1, self.settings_pages(m)),
                R::new(rx + rw - 174, detail_y(101), 72, 34),
                12,
                400,
                MUTED,
                DT_CENTER,
            );
        }
        for (i, setting) in self.visible_settings(m).iter().enumerate() {
            let y = detail_y(169 + i as i32 * 78);
            c.text(
                setting.title,
                R::new(cx, y, 210, 27),
                FONT_MD,
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
            c.line(cx, y + 58, rx + rw - 30, y + 58, BORDER, 1.);
        }
        if audio && self.audio_page == 1 {
            self.eq_panel(c, m);
        }
        let notice_y = if audio && self.audio_page == 1 {
            h - 166
        } else {
            h - 182
        };
        c.round(R::new(cx, notice_y, rw - 60, 60), RADIUS_MD, FIELD, None);
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
            R::new(cx + 18, notice_y + 10, rw - 96, 24),
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
            R::new(cx + 18, notice_y + 34, rw - 96, 20),
            12,
            400,
            DIM,
            DT_LEFT,
        );
    }
    pub(super) unsafe fn preferences(&self, c: &Canvas) {
        let left = SIDEBAR + 24;
        let width = self.width - left - 48;
        c.text(
            "偏好设置",
            R::new(left, 88, 500, 36),
            24,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "刷新、低电提醒与 Windows 启动行为",
            R::new(left + 2, 126, width - 4, 22),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        c.round(R::new(left, 168, width, 532), 14, PANEL, Some(BORDER));
        for (y, title, hint) in [
            (202, "自动刷新", "选择读取间隔，较长间隔可减少设备唤醒"),
            (297, "低电阈值", "仅在未充电且读数有效时提醒"),
            (392, "低电提醒", "同一低电周期只提醒一次"),
            (487, "设备诊断", "仅导出本地 HID 元数据"),
            (576, "开机启动", "登录 Windows 后在托盘运行"),
        ] {
            c.text(title, R::new(left + 24, y, 210, 25), 14, 600, TEXT, DT_LEFT);
            c.text(
                hint,
                R::new(left + 24, y + 25, width - 48, 21),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            if y < 550 {
                c.line(left + 24, y + 90, left + width - 24, y + 90, BORDER, 1.);
            }
        }
    }
    pub(super) unsafe fn draw_button(&self, item: &DRAWITEMSTRUCT) {
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
            if let Command::Device(m) | Command::System(m) = b.command {
                if matches!(b.command, Command::Device(_)) {
                    self.card(&c, r, m);
                } else if let Some(device) = self.snapshot.system_devices.get(m) {
                    c.soft_shadow(r, 14);
                    c.round(r, 14, PANEL, Some(BORDER));
                    c.text(
                        "WINDOWS · 只读",
                        R::new(24, 22, r.w - 48, 25),
                        12,
                        500,
                        DIM,
                        DT_LEFT,
                    );
                    c.text(
                        &device.name,
                        R::new(24, 64, r.w - 48, 72),
                        22,
                        600,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        &system_reading(device),
                        R::new(24, r.h - 115, r.w - 48, 55),
                        30,
                        500,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text(
                        "查看电量  ›",
                        R::new(24, r.h - 55, r.w - 48, 30),
                        13,
                        400,
                        MUTED,
                        DT_RIGHT,
                    );
                }
                if hover
                    || self.fx.widget.hover(widget_key(b.command))
                    || item.itemState & ODS_FOCUS != 0
                {
                    c.round(
                        R::new(3, 3, b.rect.w - 6, b.rect.h - 6),
                        14,
                        0,
                        Some(if item.itemState & ODS_FOCUS != 0 {
                            RED
                        } else {
                            crate::paint::blend(
                                PANEL,
                                crate::ui::widgets::HOVER,
                                self.fx.widget.value(widget_key(b.command)),
                            )
                        }),
                    );
                }
            }
            return;
        }
        if b.style == ENTRY {
            c.rect(R::new(0, 0, b.rect.w, b.rect.h), 0xff111117);
            let active = match b.command {
                Command::Home => {
                    matches!(self.page, Page::Home | Page::Device(_) | Page::System(_))
                }
                Command::DriverManager => false,
                Command::CoreUpdate => false,
                Command::Prefs => self.page == Page::Preferences,
                Command::Device(m) => self.page == Page::Device(m),
                Command::System(m) => self.page == Page::System(m),
                _ => false,
            };
            let key = widget_key(b.command);
            let over = hover || self.fx.widget.hover(key);
            let fill = if active {
                0xff29232b
            } else if over {
                crate::paint::blend(PANEL, crate::ui::widgets::HOVER, self.fx.widget.value(key))
            } else {
                0xff111117
            };
            c.round(
                r,
                8,
                fill,
                Some(if active { 0xff4a3038 } else { 0xff111117 }),
            );
            if active {
                c.round(R::new(2, 8, 3, r.h - 16), 1, RED, None);
            }
            match b.command {
                Command::Device(m) => {
                    let percent = self
                        .fx
                        .widget
                        .ring_value(0x1000 + m as u64)
                        .map(|v| v.round() as u8);
                    mini_ring(
                        &c,
                        R::new(10, 7, 24, 24),
                        percent,
                        self.snapshot.devices[m].fresh,
                    );
                    c.ellipse(
                        R::new(r.w - 14, 16, 6, 6),
                        connection_color(&self.snapshot.devices[m]),
                    );
                    c.text(
                        &b.label,
                        R::new(42, 4, r.w - 62, r.h - 8),
                        12,
                        500,
                        if active { TEXT } else { MUTED },
                        DT_LEFT,
                    );
                }
                Command::System(m) => {
                    let percent = self
                        .fx
                        .widget
                        .ring_value(0x8000 + m as u64)
                        .map(|v| v.round() as u8);
                    mini_ring(&c, R::new(10, 7, 24, 24), percent, true);
                    c.ellipse(R::new(r.w - 14, 16, 6, 6), crate::paint::GREEN);
                    c.text(
                        &b.label,
                        R::new(42, 4, r.w - 62, r.h - 8),
                        12,
                        500,
                        if active { TEXT } else { MUTED },
                        DT_LEFT,
                    );
                }
                _ => {
                    c.text(
                        &b.label,
                        R::new(14, 3, r.w - 28, r.h - 6),
                        13,
                        500,
                        if active { TEXT } else { MUTED },
                        DT_LEFT,
                    );
                }
            }
            if item.itemState & ODS_FOCUS != 0 {
                c.round(R::new(3, 3, b.rect.w - 6, b.rect.h - 6), 7, 0, Some(MUTED));
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
        c.soft_shadow(r, 14);
        c.round(
            r,
            RADIUS_LG,
            if self.fx.widget.hover(0x1000 + m as u64) {
                crate::paint::blend(
                    PANEL,
                    crate::ui::widgets::HOVER,
                    self.fx.widget.value(0x1000 + m as u64),
                )
            } else if active {
                PANEL
            } else {
                0xff101014
            },
            Some(if active { 0xff39313a } else { 0xff222229 }),
        );
        let margin = 22;
        c.text(
            &format!("{}  ·  {}", spec.brand, kind_name(spec.kind)),
            R::new(margin, 16, r.w - 132, 24),
            12,
            500,
            MUTED,
            DT_LEFT,
        );
        c.ellipse(R::new(r.w - 88, 25, 7, 7), connection_color(s));
        c.text(
            if active {
                "已连接"
            } else if s.present {
                "待更新"
            } else {
                "未连接"
            },
            R::new(r.w - 76, 16, 56, 24),
            11,
            500,
            connection_color(s),
            DT_RIGHT,
        );
        let image_h = (r.h - 143).max(70);
        c.image(
            self.images.images[m],
            R::new(margin, 44, r.w - 2 * margin, image_h),
            active,
        );
        c.text(
            spec.name,
            R::new(margin, r.h - 93, r.w - 2 * margin, 29),
            18,
            600,
            if active { TEXT } else { MUTED },
            DT_LEFT,
        );
        c.line(margin, r.h - 55, r.w - margin, r.h - 55, BORDER, 1.);
        c.text(
            &reading(s, spec),
            R::new(margin, r.h - 46, r.w - 145, 30),
            15,
            500,
            if active { TEXT } else { DIM },
            DT_LEFT,
        );
        c.text(
            "查看设备  ›",
            R::new(r.w - 118, r.h - 46, 96, 30),
            12,
            500,
            MUTED,
            DT_RIGHT,
        );
    }
}

unsafe fn mini_ring(c: &Canvas, r: R, percent: Option<u8>, active: bool) {
    c.arc(r, -90., 359.9, BORDER, 2.5);
    if let Some(percent) = percent {
        c.arc(
            r,
            -90.,
            360. * percent.min(100) as f32 / 100.,
            if active { RED } else { DIM },
            2.5,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_readings_keep_missing_and_cached_values_distinct() {
        let descriptor = |key: &str, kind: &str| {
            serde_json::from_value::<crate::devices::DeviceManifest>(serde_json::json!({
                "key":key,"name":"Test device","kind":kind,"brand":"Test","matches":[{"vid":1,"pid":1}]
            })).unwrap().into_spec().unwrap()
        };
        let spec = &descriptor("test-keyboard", "keyboard");
        let mut state = backend::State {
            fresh: true,
            present: true,
            connection_path: Some("蓝牙".into()),
            ..Default::default()
        };
        assert_eq!(connection(&state, true), "蓝牙");
        assert!(!reading(&state, spec).contains('%'));
        state.percent = Some(0);
        assert_eq!(reading(&state, spec), "0%");
        state.fresh = false;
        assert_eq!(reading(&state, spec), "上次 0%");
        let audio = &descriptor("hecate-g2-pro", "headset");
        state.fresh = true;
        state.percent = Some(72);
        assert_eq!(reading(&state, audio), "72%");
        state.percent = None;
        assert_eq!(reading(&state, audio), "USB 供电");
    }
}
