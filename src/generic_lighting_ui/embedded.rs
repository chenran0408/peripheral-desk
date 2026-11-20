use super::*;
use crate::controls_ui::viewport;
use crate::paint::palette;

pub(super) const WIDTH: i32 = 728;
pub(super) const HEIGHT: i32 = 520;

thread_local! {
    static PAGE: viewport::Embedded<Dialog> = const { viewport::Embedded::new() };
}

impl viewport::EditorState for Dialog {
    fn blocks_close(&self) -> bool {
        self.color_open || (self.pending.is_some() && !matches!(self.operation, Operation::Read))
    }
}

pub(crate) unsafe fn open_embedded(owner: HWND, model: usize, bounds: RECT) -> Result<(), String> {
    PAGE.with(|page| page.open(owner, model, bounds, |host| create(host, model, true)))
}

pub(crate) unsafe fn resize_embedded(bounds: RECT) {
    PAGE.with(|page| page.resize(bounds));
}

pub(crate) unsafe fn embedded_writing() -> bool {
    PAGE.with(|page| page.busy())
}

pub(crate) unsafe fn close_embedded() -> bool {
    PAGE.with(|page| page.close())
}

pub(super) unsafe fn destroyed(hwnd: HWND, data: *mut Dialog) {
    PAGE.with(|page| page.destroyed(hwnd, data));
}

impl Dialog {
    pub(super) fn embedded_rect(&self, id: usize, default: R) -> R {
        match id {
            TOGGLE => R::new(546, 94, 140, 34),
            COLOR => R::new(245, 245, 100, 34),
            CLOCKWISE => R::new(484, 245, 92, 34),
            COUNTERCLOCKWISE => R::new(584, 245, 102, 34),
            SPEED_DOWN => R::new(210, 307, 38, 32),
            SPEED_UP => R::new(254, 307, 38, 32),
            BRIGHTNESS_DOWN => R::new(604, 307, 38, 32),
            BRIGHTNESS_UP => R::new(648, 307, 38, 32),
            REFRESH => R::new(468, 466, 100, 36),
            APPLY => R::new(580, 466, 124, 36),
            _ => {
                let Some(index) = self
                    .capabilities
                    .modes
                    .iter()
                    .position(|m| MODE_BASE + m.id as usize == id)
                else {
                    return default;
                };
                R::new(
                    116 + (index % 3) as i32 * 192,
                    147 + (index / 3) as i32 * 40,
                    184,
                    34,
                )
            }
        }
    }

    pub(super) unsafe fn paint_embedded(&self, canvas: &Canvas) {
        canvas.rect(R::new(0, 0, WIDTH, HEIGHT), palette().bg);
        canvas.text(
            "灯效设置",
            R::new(24, 14, 380, 39),
            25,
            600,
            palette().text,
            DT_LEFT,
        );
        canvas.text(
            &self.name,
            R::new(24, 54, 650, 23),
            14,
            400,
            palette().muted,
            DT_LEFT,
        );
        canvas.round(
            R::new(24, 82, 680, 276),
            12,
            palette().panel,
            Some(palette().border),
        );
        let switch = match self.actual_enabled {
            Some(true) => "当前开启",
            Some(false) => "当前关闭",
            None => "尚未读取",
        };
        let label = |title: &str, x, y| {
            canvas.text(
                title,
                R::new(x, y, 80, 26),
                13,
                500,
                palette().muted,
                DT_LEFT,
            )
        };
        label("灯效开关", 42, 98);
        canvas.text(
            switch,
            R::new(116, 98, 165, 26),
            15,
            500,
            palette().text,
            DT_LEFT,
        );
        label("模式", 42, 151);
        label("颜色", 42, 249);
        if let Some(config) = self
            .config
            .as_ref()
            .filter(|c| self.mode(c.effect).is_some_and(|mode| mode.color))
        {
            let [r, g, b] = config.rgb;
            let color = u32::from_be_bytes([255, r, g, b]);
            canvas.round(R::new(116, 249, 30, 26), 6, color, Some(palette().border));
            canvas.text(
                &format!("#{r:02X}{g:02X}{b:02X}"),
                R::new(154, 249, 86, 26),
                14,
                500,
                palette().text,
                DT_LEFT,
            );
        } else {
            canvas.text(
                "—",
                R::new(116, 249, 110, 26),
                14,
                500,
                palette().dim,
                DT_LEFT,
            );
        }
        if self.show_direction() {
            label("流动方向", 401, 249);
        }
        let config = self.config.as_ref();
        for (title, x, value, limit) in [
            (
                "速度",
                42,
                config.map(|c| c.speed),
                self.capabilities.speed_max,
            ),
            (
                "亮度",
                401,
                config.map(|c| c.brightness),
                self.capabilities.brightness_max,
            ),
        ] {
            label(title, x, 310);
            let current = value.map_or_else(|| "—".into(), |v| format!("{v} / {limit}"));
            canvas.text(
                &current,
                R::new(x + 74, 310, 90, 26),
                14,
                500,
                palette().text,
                DT_LEFT,
            );
        }
        canvas.text(
            self.config_note(),
            R::new(24, 374, 680, 35),
            13,
            400,
            palette().dim,
            DT_LEFT | DT_WORDBREAK,
        );
        canvas.text(
            &self.status,
            R::new(24, 412, 680, 50),
            13,
            400,
            if self.error {
                palette().error
            } else {
                palette().muted
            },
            DT_LEFT | DT_WORDBREAK,
        );
    }
}
