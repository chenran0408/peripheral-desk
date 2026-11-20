//! Painting for battery monitoring, device controls and application preferences.
use super::widgets::{FONT_LG, FONT_MD, RADIUS_LG, RADIUS_MD};
use super::{widget_key, App, Command, Page, CARD, ENTRY, GHOST, HOME_CONTENT_Y, SIDEBAR};
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
        c.text(
            kind_name(spec.kind),
            R::new(panel.x + 14, panel.y + 264, panel.w - 28, 24),
            12,
            400,
            MUTED,
            DT_LEFT,
        );
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
        c.gradient_v(
            R::new(SIDEBAR + 1, 0, self.width - SIDEBAR - 1, 66),
            crate::paint::blend(BG, 0xff25131a, self.fx.nav.value()),
            BG,
        );
        c.line(SIDEBAR + 24, 65, self.width - 28, 65, 0xff19191f, 1.);
    }

    unsafe fn page_heading(&self, c: &Canvas, title: &str, description: &str) {
        let left = SIDEBAR + 24;
        let width = self.width - left - 28;
        c.text(title, R::new(left, 84, width, 38), 26, 600, TEXT, DT_LEFT);
        c.text(
            description,
            R::new(left + 1, 125, width, 22),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
    }

    pub(super) unsafe fn home(&self, c: &Canvas) {
        self.page_heading(c, "电量", "查看外设的剩余电量、充电状态和上次读数。");
        if !self.battery_entries().is_empty() {
            return;
        }
        let panel = R::new(
            SIDEBAR + 24,
            HOME_CONTENT_Y,
            self.width - SIDEBAR - 52,
            self.height - HOME_CONTENT_Y - 86,
        );
        let center_y = panel.y + panel.h / 2;
        c.round(panel, RADIUS_LG, PANEL, Some(BORDER));
        c.text(
            "暂无电量设备",
            R::new(panel.x + 24, center_y - 98, panel.w - 48, 42),
            24,
            600,
            TEXT,
            DT_CENTER,
        );
        c.text(
            "连接蓝牙设备或手柄，或为你的外设安装电量插件。",
            R::new(panel.x + 24, center_y - 50, panel.w - 48, 30),
            14,
            400,
            MUTED,
            DT_CENTER,
        );
    }

    pub(super) unsafe fn drivers(&self, c: &Canvas) {
        self.page_heading(
            c,
            "设备调节",
            "迈从设备使用内置 M HUB，其他设备选择对应的型号。",
        );
    }

    fn battery_view(&self, command: Command) -> Option<BatteryView<'_>> {
        match command {
            Command::Battery(m) => {
                let spec = REGISTRY.get(m)?;
                let state = self.snapshot.devices.get(m)?;
                Some(BatteryView {
                    name: spec.name,
                    category: format!("{} · {}", spec.brand, kind_name(spec.kind)),
                    state,
                    value: state
                        .percent
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "未知".into()),
                    link: connection(state, self.images.installed[m]),
                    source: if spec.plugin_version.is_empty() {
                        "设备插件".into()
                    } else {
                        format!("设备插件 {}", spec.plugin_version)
                    },
                    model: Some(m),
                    is_level: false,
                })
            }
            Command::System(m) => {
                let device = self.snapshot.system_devices.get(m)?;
                let link = if device.state.key.starts_with("system-bt-") {
                    "蓝牙"
                } else {
                    "Windows 已连接"
                };
                Some(BatteryView {
                    name: &device.name,
                    category: "Windows 系统设备".into(),
                    state: &device.state,
                    value: system_reading(device),
                    link,
                    source: "Windows 系统电量".into(),
                    model: None,
                    is_level: device.level.is_some(),
                })
            }
            _ => None,
        }
    }

    pub(super) unsafe fn battery_detail(&self, c: &Canvas, m: usize) {
        self.paint_battery_detail(c, Command::Battery(m));
    }
    pub(super) unsafe fn system_detail(&self, c: &Canvas, m: usize) {
        self.paint_battery_detail(c, Command::System(m));
    }

    unsafe fn paint_battery_detail(&self, c: &Canvas, command: Command) {
        let Some(battery) = self.battery_view(command) else {
            return;
        };
        self.page_heading(c, battery.name, &battery.category);
        let x = SIDEBAR + 24;
        let w = self.width - x - 28;
        let y = HOME_CONTENT_Y;
        let panel = R::new(x, y, w, 176);
        c.round(panel, RADIUS_LG, PANEL, Some(BORDER));
        c.text(
            "剩余电量",
            R::new(x + 28, y + 22, w - 260, 25),
            14,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            &battery.value,
            R::new(x + 26, y + 51, w - 265, 70),
            44,
            500,
            TEXT,
            DT_LEFT,
        );
        c.text(
            battery.sample_label(),
            R::new(x + 28, y + 128, w - 265, 24),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        if let Some(m) = battery.model {
            c.image(
                self.images.images[m],
                R::new(x + w - 229, y + 16, 198, 144),
                battery.state.fresh,
            );
        }
        let rows_top = y + panel.h + 16;
        let rows = [
            ("连接状态", battery.link.to_owned()),
            ("供电状态", battery.power_label().into()),
            ("最近读取", reading_age(battery.state.updated)),
            ("数据来源", battery.source),
        ];
        let row_h = 55;
        c.round(
            R::new(x, rows_top, w, rows.len() as i32 * row_h + 12),
            RADIUS_LG,
            PANEL,
            Some(BORDER),
        );
        for (i, (label, value)) in rows.iter().enumerate() {
            let y = rows_top + 6 + i as i32 * row_h;
            if i > 0 {
                c.line(x + 24, y, x + w - 24, y, BORDER, 1.);
            }
            c.text(
                label,
                R::new(x + 26, y, 130, row_h),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                value,
                R::new(x + 172, y, w - 200, row_h),
                15,
                500,
                TEXT,
                DT_LEFT,
            );
        }
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
            "管理电量提醒、配置备份和 Windows 启动行为。",
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
            (
                487,
                "工具与数据",
                "备份本地配置、导出诊断或管理由外设盒运行的软件宏",
            ),
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
            let over = hover || self.fx.widget.hover(widget_key(b.command));
            let fill = if over {
                crate::ui::widgets::HOVER
            } else {
                PANEL
            };
            let focus = item.itemState & ODS_FOCUS != 0;
            c.round(r, RADIUS_LG, fill, Some(if focus { RED } else { BORDER }));
            if let Some(battery) = self.battery_view(b.command) {
                self.paint_battery_card(&c, r, &battery);
            } else {
                self.paint_driver_card(&c, r, b.command);
            }
            return;
        }
        if b.style == ENTRY {
            c.rect(R::new(0, 0, b.rect.w, b.rect.h), 0xff111117);
            let active = match b.command {
                Command::Home => {
                    matches!(self.page, Page::Home | Page::Battery(_) | Page::System(_))
                }
                Command::Drivers => matches!(
                    self.page,
                    Page::Drivers | Page::MHub | Page::Onboard(_) | Page::Device(_)
                ),
                Command::Prefs => self.page == Page::Preferences,
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
            c.text(
                &b.label,
                R::new(14, 3, r.w - 28, r.h - 6),
                14,
                if active { 600 } else { 500 },
                if active { TEXT } else { MUTED },
                DT_LEFT,
            );
            if item.itemState & ODS_FOCUS != 0 {
                c.round(R::new(3, 3, b.rect.w - 6, b.rect.h - 6), 7, 0, Some(MUTED));
            }
            return;
        }
        let inside_panel = match self.page {
            Page::Home => b.command == Command::AddDevice,
            Page::Preferences => (168..700).contains(&b.rect.y),
            Page::Device(_) => (145..self.height - 82).contains(&b.rect.y),
            _ => false,
        };
        c.rect(
            R::new(0, 0, b.rect.w, b.rect.h),
            if inside_panel { PANEL } else { BG },
        );
        if b.rect.y < 65 {
            c.gradient_v(
                R::new(
                    SIDEBAR + 1 - b.rect.x,
                    -b.rect.y,
                    self.width - SIDEBAR - 1,
                    66,
                ),
                crate::paint::blend(BG, 0xff25131a, self.fx.nav.value()),
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
    unsafe fn paint_battery_card(&self, c: &Canvas, r: R, battery: &BatteryView<'_>) {
        let x = 20;
        let w = r.w - x * 2;
        let active = battery.state.fresh || battery.is_level;
        c.text(
            &battery.category,
            R::new(x, 15, w, 22),
            12,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            battery.name,
            R::new(x, 45, w, 31),
            20,
            600,
            if active { TEXT } else { MUTED },
            DT_LEFT,
        );
        let center = (80 + r.h - 55) / 2;
        let image_w = (r.w / 3).min(150);
        let value_w = if let Some(m) = battery.model {
            let image_h = (r.h - 135).min(128);
            c.image(
                self.images.images[m],
                R::new(r.w - x - image_w, center - image_h / 2, image_w, image_h),
                active,
            );
            w - image_w - 8
        } else {
            w
        };
        c.text(
            &battery.value,
            R::new(x, center - 31, value_w, 50),
            32,
            500,
            if active { TEXT } else { MUTED },
            DT_LEFT,
        );
        c.text(
            battery.sample_label(),
            R::new(x + 1, center + 18, value_w, 22),
            12,
            400,
            MUTED,
            DT_LEFT,
        );
        c.line(x, r.h - 48, r.w - x, r.h - 48, BORDER, 1.);
        c.ellipse(
            R::new(x, r.h - 27, 6, 6),
            if battery.is_level {
                GREEN
            } else {
                connection_color(battery.state)
            },
        );
        c.text(
            battery.link,
            R::new(x + 14, r.h - 41, w - 110, 33),
            12,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            "查看电量  ›",
            R::new(r.w - 110, r.h - 41, 90, 33),
            13,
            500,
            TEXT,
            DT_RIGHT,
        );
    }

    unsafe fn paint_driver_card(&self, c: &Canvas, r: R, command: Command) {
        let (category, name, capabilities, footer, button) = match command {
            Command::MHub => (
                "内置驱动中心".to_owned(),
                "迈从 M HUB",
                "在同一处选择迈从设备，编辑并保存板载配置。".to_owned(),
                "迈从设备".to_owned(),
                if self.hub_open {
                    "返回 M HUB"
                } else {
                    "打开 M HUB"
                },
            ),
            Command::Device(m) => {
                let spec = &REGISTRY[m];
                let (action, capabilities) = driver_summary(spec);
                (
                    format!("{} · {}", spec.brand, kind_name(spec.kind)),
                    spec.name,
                    capabilities,
                    connection(&self.snapshot.devices[m], self.images.installed[m]).to_owned(),
                    action,
                )
            }
            _ => return,
        };
        let x = 20;
        let w = r.w - x * 2;
        c.text(&category, R::new(x, 15, w, 22), 12, 500, MUTED, DT_LEFT);
        c.text(name, R::new(x, 45, w, 38), 22, 600, TEXT, DT_LEFT);
        let mid = (84 + r.h - 48) / 2;
        c.text(
            &capabilities,
            R::new(x, mid - 19, w, 55),
            15,
            400,
            MUTED,
            DT_LEFT | DT_WORDBREAK,
        );
        c.line(x, r.h - 48, r.w - x, r.h - 48, BORDER, 1.);
        c.text(
            &footer,
            R::new(x, r.h - 41, w - 140, 33),
            12,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            &format!("{button}  ›"),
            R::new(r.w - 158, r.h - 41, 138, 33),
            13,
            500,
            TEXT,
            DT_RIGHT,
        );
    }
}

struct BatteryView<'a> {
    name: &'a str,
    category: String,
    state: &'a backend::State,
    value: String,
    link: &'a str,
    source: String,
    model: Option<usize>,
    is_level: bool,
}

impl BatteryView<'_> {
    fn sample_label(&self) -> &str {
        if self.is_level {
            "设备上报等级"
        } else if !self.state.fresh && self.state.percent.is_some() {
            "保留的上次读数"
        } else if self.state.charging.is_some() && self.state.fresh {
            self.power_label()
        } else if self.state.percent.is_none() {
            "暂无百分比读数"
        } else {
            "最近一次读取"
        }
    }

    fn power_label(&self) -> &str {
        if self.model.is_none() {
            match self.state.charging {
                Some(true) => "正在充电",
                Some(false) => "电池供电",
                None => "未知",
            }
        } else {
            power(self.state)
        }
    }
}

fn reading_age(updated: u64) -> String {
    if updated == 0 {
        return "暂无记录".into();
    }
    match backend::now().saturating_sub(updated) {
        0..=59 => "不到 1 分钟前".into(),
        seconds @ 60..=3599 => format!("{} 分钟前", seconds / 60),
        seconds @ 3600..=86399 => format!("{} 小时前", seconds / 3600),
        seconds => format!("{} 天前", seconds / 86400),
    }
}

fn driver_summary(spec: &crate::devices::DeviceSpec) -> (&'static str, String) {
    let mut features = Vec::new();
    if spec.supports("onboard-profiles") {
        features.push("板载配置与 DPI 档");
    }
    if spec.has_g2_audio() {
        features.extend(["音效", "均衡器"]);
    }
    if spec.dpi_range.is_some() {
        features.push("DPI");
    }
    if !spec.rates.is_empty() || !spec.wired_rates.is_empty() {
        features.push("回报率");
    }
    if !spec.sleeps.is_empty() {
        features.push("休眠");
    }
    if spec.supports("bindings") {
        features.push("改键");
    }
    if spec.supports("lighting-editor")
        || spec.supports("lighting-switch")
        || spec.supports("key-colors")
    {
        features.push("灯光");
    }
    if features.is_empty() {
        if spec.supports("dpi-stages-read") || spec.supports("polling-rate-read") {
            return ("查看参数", "读取设备上报的指针参数。".into());
        }
        return ("打开设置", "打开此型号的设备设置。".into());
    }
    ("打开设置", features.join("  ·  "))
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
