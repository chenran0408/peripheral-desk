use super::*;

pub(super) const WIDTH: i32 = 728;
pub(super) const HEIGHT: i32 = 476;

thread_local! {
    static PAGE: viewport::Embedded<Editor> = const { viewport::Embedded::new() };
}

impl viewport::EditorState for Editor {
    fn blocks_close(&self) -> bool {
        (self.pending.is_some() && self.saving) || self.color_open
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

pub(super) unsafe fn destroyed(hwnd: HWND, data: *mut Editor) {
    PAGE.with(|page| page.destroyed(hwnd, data));
}

pub(super) fn control_rect(id: i32, default: R) -> R {
    match id {
        KEYS => R::new(24, 108, 216, 251),
        KIND => R::new(276, 156, 312, 250),
        TARGET => R::new(276, 225, 384, 300),
        id if MODS.contains(&id) => R::new(276 + (id - MODS[0]) * 96, 274, 90, 28),
        RELOAD => R::new(468, 422, 100, 36),
        APPLY => R::new(580, 422, 124, 36),
        STATUS => R::new(24, 376, 680, 38),
        _ => default,
    }
}

impl Editor {
    pub(super) unsafe fn paint_embedded(&self, canvas: &Canvas) {
        canvas.rect(R::new(0, 0, WIDTH, HEIGHT), BG);
        canvas.text("按键设置", R::new(24, 14, 360, 39), 25, 600, TEXT, DT_LEFT);
        let subtitle = self
            .state
            .as_ref()
            .and_then(|state| state.profile)
            .map_or_else(
                || REGISTRY[self.model].name.to_owned(),
                |profile| {
                    format!(
                        "{}   ·   板载配置 {}",
                        REGISTRY[self.model].name,
                        u16::from(profile) + 1
                    )
                },
            );
        canvas.text(&subtitle, R::new(24, 54, 650, 23), 14, 400, MUTED, DT_LEFT);
        canvas.text(
            "物理按键 / 分层",
            R::new(24, 80, 216, 22),
            14,
            600,
            TEXT,
            DT_LEFT,
        );
        canvas.round(R::new(254, 80, 450, 280), 12, PANEL, Some(BORDER));
        let binding = self.binding();
        let title = binding
            .map(|binding| format!("{} · 第 {} 层", binding.label, binding.layer))
            .unwrap_or_else(|| "尚无可用键位".into());
        canvas.text(&title, R::new(276, 89, 400, 28), 18, 600, TEXT, DT_LEFT);
        canvas.text(
            "操作类型",
            R::new(276, 128, 300, 22),
            13,
            500,
            MUTED,
            DT_LEFT,
        );
        canvas.text(
            "目标功能 / 按键",
            R::new(276, 198, 372, 22),
            13,
            500,
            MUTED,
            DT_LEFT,
        );
        if let Some(binding) = binding {
            let access = if self.editable() { "" } else { "（只读）" };
            canvas.text(
                &format!("设备当前设置{access}   {}", self.describe(&binding.action)),
                R::new(276, 312, 408, 36),
                13,
                400,
                MUTED,
                DT_LEFT | DT_WORDBREAK,
            );
        }
    }
}
