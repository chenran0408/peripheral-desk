//! Page painting for the main window: home dashboard, device detail and preferences.
use super::{App, Command, Page, CARD, CHIP, ENTRY, GHOST, HOME_CONTENT_Y};
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
pub(crate) struct NavFx {}

impl NavFx {
    pub(crate) fn tick(&mut self, _hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        false
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
pub(super) fn connection(s: &backend::State, installed: bool) -> &'static str {
    if !installed {
        return "请从驱动仓库安装插件";
    }
    if s.fresh {
        if s.wired {
            "USB 有线连接"
        } else {
            "2.4 GHz 无线连接"
        }
    } else if s.present {
        if REGISTRY
            .iter()
            .any(|spec| spec.key == s.key && matches!(spec.kind, Kind::Headset | Kind::Accessory))
        {
            "USB 已连接 · 状态未就绪"
        } else {
            "接收器在线 · 待唤醒"
        }
    } else {
        "设备未连接"
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
    pub(super) fn eq_hit(&self, x: i32, y: i32) -> Option<(Tuning, R)> {
        if !self.can_adjust() || self.audio_page != 1 {
            return None;
        }
        let Page::Device(m) = self.page else {
            return None;
        };
        if REGISTRY[m].kind != Kind::Headset {
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
        c.gradient(R::new(0, 0, self.width * 2 / 3, 68), 0xff261119, BG);
        c.line(28, 65, self.width - 28, 65, 0xff19191f, 1.);
        c.ellipse(
            R::new(if self.page == Page::Home { 28 } else { 186 }, 31, 7, 7),
            RED,
        );
        c.text(
            if self.page == Page::Home {
                "外设盒  /  外设控制中心"
            } else {
                "外设盒"
            },
            R::new(
                if self.page == Page::Home { 47 } else { 206 },
                18,
                if self.page == Page::Home { 450 } else { 75 },
                32,
            ),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
    }
    pub(super) unsafe fn home(&self, c: &Canvas) {
        c.text("我的设备", R::new(32, 154, 600, 40), 27, 600, TEXT, DT_LEFT);
        c.text(
            if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
                "尚未加载设备驱动，先从驱动商店添加你的设备"
            } else if REGISTRY.is_empty() {
                "选择设备，查看 Windows 提供的电量"
            } else {
                "选择设备，查看电量并调整设置"
            },
            R::new(34, 199, 700, 23),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        if REGISTRY.is_empty() && self.snapshot.system_devices.is_empty() {
            let center_y = HOME_CONTENT_Y + (self.height - HOME_CONTENT_Y - 86) / 2;
            c.round(
                R::new(
                    32,
                    HOME_CONTENT_Y,
                    self.width - 64,
                    self.height - HOME_CONTENT_Y - 86,
                ),
                20,
                PANEL,
                Some(BORDER),
            );
            c.text(
                "从添加设备驱动开始",
                R::new(60, center_y - 98, self.width - 120, 42),
                28,
                600,
                TEXT,
                DT_CENTER,
            );
            c.text(
                "驱动按设备下载，只安装你需要的型号。",
                R::new(60, center_y - 44, self.width - 120, 26),
                15,
                400,
                MUTED,
                DT_CENTER,
            );
            c.text(
                "安装新设备驱动后，请退出并重新打开主程序。",
                R::new(60, center_y + 76, self.width - 120, 26),
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
        c.text(
            &device.name,
            R::new(40, 165, self.width - 80, 64),
            28,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            "Windows 系统电量 · 只读",
            R::new(42, 230, self.width - 84, 30),
            14,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            &system_reading(device),
            R::new(42, 305, self.width - 84, 95),
            54,
            500,
            TEXT,
            DT_LEFT,
        );
        if device.state.charging == Some(true) {
            c.text(
                "正在充电",
                R::new(42, 416, self.width - 84, 28),
                15,
                400,
                MUTED,
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
        c.round(R::new(28, 145, 344, h - 227), 20, PANEL, Some(BORDER));
        c.round(R::new(rx, 145, rw, h - 227), 20, PANEL, Some(BORDER));
        if s.fresh {
            c.glow(200, 225, 116);
        }
        c.image(
            self.images.images[m],
            R::new(60, detail_y(100), 280, 162),
            s.fresh,
        );
        c.text(
            spec.name,
            R::new(54, detail_y(308), 300, 32),
            20,
            600,
            if s.fresh { TEXT } else { MUTED },
            DT_LEFT,
        );
        c.ellipse(R::new(56, detail_y(354), 7, 7), connection_color(s));
        c.text(
            connection(s, self.images.installed[m]),
            R::new(74, detail_y(340), 272, 32),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        c.text(
            spec.validation,
            R::new(54, detail_y(370), 292, 17),
            10,
            400,
            DIM,
            DT_LEFT,
        );
        c.line(54, detail_y(393), 346, detail_y(393), BORDER, 1.);
        if spec.kind == Kind::Accessory {
            c.text(
                "USB 供电 · 无电池",
                R::new(54, detail_y(415), 292, 30),
                18,
                500,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "灯效控制",
                R::new(424, detail_y(101), rw - 60, 32),
                22,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "点击顶部“灯效设置”读取并编辑设备灯光。",
                R::new(424, detail_y(150), rw - 60, 30),
                15,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                "仅开放插件声明的模式、颜色与调节范围。",
                R::new(424, detail_y(194), rw - 60, 30),
                13,
                400,
                DIM,
                DT_LEFT,
            );
            return;
        }
        if spec.kind == Kind::Headset {
            for (i, (label, key)) in [
                ("GAMING", Tuning::GameVolume),
                ("CHAT", Tuning::ChatVolume),
                ("麦克风", Tuning::MicVolume),
            ]
            .iter()
            .enumerate()
            {
                let y = detail_y(404 + i as i32 * 34);
                c.text(label, R::new(54, y, 150, 25), 13, 400, DIM, DT_LEFT);
                c.text(
                    &self
                        .tuning_value(m, *key)
                        .map(|v| format!("{v}%"))
                        .unwrap_or_else(|| "不可用".into()),
                    R::new(210, y, 136, 25),
                    13,
                    400,
                    MUTED,
                    DT_RIGHT,
                );
            }
            let y = (h - 312).max(520);
            c.round(R::new(112, y, 176, 112), 20, FIELD, Some(BORDER));
            c.text(
                "USB",
                R::new(112, y + 13, 176, 48),
                34,
                300,
                if s.fresh { TEXT } else { DIM },
                DT_CENTER,
            );
            c.text(
                "有线供电 · 无电池",
                R::new(90, y + 72, 220, 24),
                13,
                400,
                MUTED,
                DT_CENTER,
            );
            self.advanced_panel(c, m);
            return;
        }
        let pairs = [
            (
                "当前 DPI",
                s.dpi
                    .map(|v| format_number(v as u32))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "回报率",
                s.rate
                    .map(|v| format!("{v} Hz"))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "自动休眠",
                s.sleep_seconds
                    .map(|v| sleep_label(v, true))
                    .unwrap_or_else(|| "—".into()),
            ),
            ("供电状态", power(s).into()),
        ];
        for (i, (label, value)) in pairs.iter().enumerate() {
            if spec.kind == Kind::Keyboard && i == 0 {
                continue;
            }
            let y = detail_y(400 + i as i32 * 31);
            c.text(label, R::new(54, y, 120, 25), 13, 400, DIM, DT_LEFT);
            c.text(value, R::new(168, y, 178, 25), 13, 400, MUTED, DT_RIGHT);
        }
        let compact_ring = h < 800;
        let ring_y = if compact_ring {
            526
        } else {
            (h - 303).max(538)
        };
        c.ring(
            R::new(
                if compact_ring { 140 } else { 133 },
                ring_y,
                if compact_ring { 120 } else { 134 },
                if compact_ring { 120 } else { 134 },
            ),
            s.percent.filter(|_| s.fresh),
            s.fresh,
        );
        c.text(
            if s.fresh {
                if s.charging == Some(true) {
                    "正在充电"
                } else if s.percent.is_some_and(|p| p < 20) {
                    "电量偏低"
                } else {
                    "电量充足"
                }
            } else {
                "等待设备连接"
            },
            R::new(60, ring_y + if compact_ring { 136 } else { 150 }, 280, 24),
            13,
            400,
            MUTED,
            DT_CENTER,
        );
        if spec.read_only() {
            c.text(
                "只读设备状态",
                R::new(cx, detail_y(104), rw - 60, 34),
                23,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                spec.validation,
                R::new(cx, detail_y(152), rw - 60, 25),
                13,
                400,
                AMBER,
                DT_LEFT,
            );
            c.round(
                R::new(cx, detail_y(205), rw - 60, 134),
                16,
                FIELD,
                Some(BORDER),
            );
            c.text(
                "此插件提供设备识别与电量读取。",
                R::new(cx + 22, detail_y(225), rw - 104, 28),
                16,
                500,
                TEXT,
                DT_LEFT,
            );
            c.text(
                "DPI、改键、宏和灯效写入未开放。",
                R::new(cx + 22, detail_y(268), rw - 104, 26),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.text(
                if s.fresh {
                    "当前读数已更新"
                } else {
                    &s.error
                },
                R::new(cx, detail_y(365), rw - 60, 70),
                13,
                400,
                if s.fresh { GREEN } else { DIM },
                DT_LEFT,
            );
            return;
        }
        if self.advanced && !spec.advanced.is_empty() {
            self.advanced_panel(c, m);
            return;
        }
        if spec.kind == Kind::Mouse {
            c.text(
                "DPI 灵敏度",
                R::new(cx, detail_y(101), 300, 30),
                20,
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
        } else {
            c.text(
                "键盘灯效",
                R::new(cx, detail_y(101), 300, 32),
                20,
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
            "更多设备设置",
            R::new(cx, detail_y(513) + lower_section, 200, 23),
            15,
            500,
            MUTED,
            DT_LEFT,
        );
        c.text(
            if spec.advanced.is_empty() {
                "更多原驱动功能将逐步接入"
            } else {
                "点击上方「高级设置」，调整更多设备参数"
            },
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
        let audio = REGISTRY[m].kind == Kind::Headset;
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
            22,
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
                15,
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
        c.round(R::new(cx, notice_y, rw - 60, 60), 12, FIELD, None);
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
        c.text("工具设置", R::new(40, 155, 500, 40), 27, 600, TEXT, DT_LEFT);
        c.round(
            R::new(32, 212, self.width - 64, 488),
            20,
            PANEL,
            Some(BORDER),
        );
        for (y, title, hint) in [
            (251, "自动刷新", "较长间隔可减少设备唤醒"),
            (344, "低电阈值", "仅在未充电且读数有效时提醒"),
            (437, "低电提醒", "同一低电周期只提醒一次"),
            (529, "设备诊断", "仅导出本地 HID 元数据"),
            (622, "开机启动", "登录 Windows 后在托盘运行"),
        ] {
            c.text(title, R::new(62, y, 240, 28), 16, 500, TEXT, DT_LEFT);
            c.text(hint, R::new(62, y + 30, 250, 25), 12, 400, DIM, DT_LEFT);
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
                    c.round(r, 8, PANEL, Some(BORDER));
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
                if hover || item.itemState & ODS_FOCUS != 0 {
                    c.round(
                        R::new(3, 3, b.rect.w - 6, b.rect.h - 6),
                        18,
                        0,
                        Some(if item.itemState & ODS_FOCUS != 0 {
                            RED
                        } else {
                            0xff66505c
                        }),
                    );
                }
            }
            return;
        }
        if b.style == ENTRY {
            c.rect(R::new(0, 0, b.rect.w, b.rect.h), BG);
            let selected = match b.command {
                Command::Home => {
                    matches!(self.page, Page::Home | Page::Device(_) | Page::System(_))
                }
                Command::Prefs => self.page == Page::Preferences,
                _ => false,
            };
            c.round(
                r,
                8,
                if hover || selected { FIELD } else { PANEL },
                Some(if selected { RED } else { BORDER }),
            );
            let hint = match b.command {
                Command::Home => "查看与调整外设",
                Command::DriverManager => "获取设备驱动",
                Command::Configuration => "备份与恢复设置",
                Command::CoreUpdate => "获取程序新版本",
                Command::Prefs => "刷新与提醒",
                _ => "",
            };
            c.text(
                &b.label,
                R::new(15, 7, r.w - 30, 26),
                15,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(hint, R::new(15, 31, r.w - 30, 19), 11, 400, DIM, DT_LEFT);
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
        c.round(
            r,
            20,
            if active { PANEL } else { 0xff101014 },
            Some(if active { 0xff39313a } else { 0xff222229 }),
        );
        c.text(
            spec.brand,
            R::new(24, 20, r.w - 110, 22),
            12,
            500,
            if active { MUTED } else { DIM },
            DT_LEFT,
        );
        c.ellipse(R::new(r.w - 72, 27, 6, 6), connection_color(s));
        c.text(
            if active {
                "已连接"
            } else if !self.images.installed[m] {
                "未安装"
            } else if s.present {
                if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "未就绪"
                } else {
                    "待唤醒"
                }
            } else {
                "未连接"
            },
            R::new(r.w - 62, 18, 56, 24),
            11,
            400,
            connection_color(s),
            DT_LEFT,
        );
        if r.h < 400 {
            let short = r.h < 280;
            if active {
                c.glow(120, r.h / 2 + 12, 83);
            }
            let image_area = match spec.kind {
                Kind::Keyboard => R::new(18, 62, 196, r.h - 92),
                Kind::Mouse => R::new(36, 58, 160, r.h - 86),
                Kind::Headset | Kind::Accessory => R::new(26, 58, 180, r.h - 86),
            };
            c.image(self.images.images[m], image_area, active);
            c.text(
                spec.name,
                R::new(222, if short { 65 } else { 72 }, r.w - 244, 34),
                if short { 17 } else { 22 },
                600,
                if active { TEXT } else { DIM },
                DT_LEFT,
            );
            c.text(
                match spec.kind {
                    Kind::Mouse => "无线游戏鼠标",
                    Kind::Keyboard => "无线机械键盘",
                    Kind::Headset => "USB 双声卡耳机",
                    Kind::Accessory => "USB 外设",
                },
                R::new(224, if short { 102 } else { 115 }, r.w - 248, 24),
                13,
                400,
                if active { MUTED } else { DIM },
                DT_LEFT,
            );
            c.text(
                spec.validation,
                R::new(224, if short { 130 } else { 145 }, r.w - 248, 22),
                11,
                400,
                DIM,
                DT_LEFT,
            );
            let divider = if short { 155 } else { r.h - 110 };
            c.line(224, divider, r.w - 24, divider, BORDER, 1.);
            c.text(
                if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "USB 有线供电"
                } else if !s.fresh && s.percent.is_some() {
                    "上次电量"
                } else {
                    "设备电量"
                },
                R::new(224, if short { 161 } else { r.h - 99 }, 180, 22),
                12,
                400,
                DIM,
                DT_LEFT,
            );
            c.text(
                &if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                    "USB".into()
                } else {
                    s.percent
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "—".into())
                },
                R::new(
                    222,
                    if short { 183 } else { r.h - 72 },
                    150,
                    if short { 34 } else { 43 },
                ),
                if short { 27 } else { 32 },
                300,
                if active { TEXT } else { DIM },
                DT_LEFT,
            );
            c.text(
                "设备设置  ›",
                R::new(r.w - 150, r.h - if short { 43 } else { 65 }, 124, 32),
                13,
                500,
                if active { RED } else { DIM },
                DT_RIGHT,
            );
            return;
        }
        if active {
            c.glow(r.w / 2, 170, 110);
        }
        c.image(self.images.images[m], R::new(34, 60, r.w - 68, 222), active);
        c.text(
            spec.name,
            R::new(24, 309, r.w - 48, 34),
            20,
            600,
            if active { TEXT } else { DIM },
            DT_LEFT,
        );
        c.text(
            match spec.kind {
                Kind::Mouse => "无线游戏鼠标",
                Kind::Keyboard => "无线机械键盘",
                Kind::Headset => "USB 双声卡耳机",
                Kind::Accessory => "USB 外设",
            },
            R::new(24, 349, r.w - 48, 24),
            12,
            400,
            if active { MUTED } else { DIM },
            DT_LEFT,
        );
        c.line(24, r.h - 116, r.w - 24, r.h - 116, BORDER, 1.);
        c.text(
            if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                "供电方式"
            } else if !s.fresh && s.percent.is_some() {
                "上次电量"
            } else {
                "电量"
            },
            R::new(24, r.h - 102, 100, 25),
            12,
            400,
            DIM,
            DT_LEFT,
        );
        c.text(
            &if matches!(spec.kind, Kind::Headset | Kind::Accessory) {
                "USB".into()
            } else {
                s.percent
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "—".into())
            },
            R::new(24, r.h - 77, 140, 46),
            32,
            300,
            if active { TEXT } else { DIM },
            DT_LEFT,
        );
        c.text(
            if active {
                "设备设置  ›"
            } else {
                "查看设备  ›"
            },
            R::new(r.w - 145, r.h - 72, 120, 36),
            13,
            500,
            if active { RED } else { DIM },
            DT_RIGHT,
        );
    }
}
