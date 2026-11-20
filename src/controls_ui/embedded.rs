use super::*;
use crate::paint::palette;

pub(super) const WIDTH: i32 = 728;
pub(super) const HEIGHT: i32 = 476;

pub(crate) const WRITE_FINISHED: u32 = WM_APP + 0xD4;
pub(super) const RETIRE: u32 = WM_APP + 0xD5;
thread_local! {
    static DETACHED: std::cell::RefCell<Vec<(HWND,usize)>>=const {std::cell::RefCell::new(Vec::new())};
    static NOTICES: std::cell::RefCell<std::collections::VecDeque<(usize,String,bool)>>=const {std::cell::RefCell::new(std::collections::VecDeque::new())};
    static PAGE: viewport::Embedded<Editor> = const { viewport::Embedded::new() };
}

impl viewport::EditorState for Editor {
    fn blocks_close(&self) -> bool {
        (self.pending.is_some() && self.saving) || self.color_open
    }
}

pub(crate) unsafe fn open_embedded(owner: HWND, model: usize, bounds: RECT) -> Result<(), String> {
    let requested = crate::plugin_manager::selected_device_target(REGISTRY[model].key);
    let same_target = |hwnd: HWND| {
        let editor = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor);
        editor.target.as_ref().map(|t| &t.id) == requested.as_ref().map(|t| &t.id)
    };
    let change = PAGE.with(|page| !page.content().is_null() && !same_target(page.content()));
    if change && !close_embedded() {
        return Err("请先关闭取色器".into());
    }
    if let Some(content) = DETACHED.with_borrow(|all| {
        all.iter()
            .find(|(h, m)| *m == model && same_target(*h))
            .map(|(h, _)| *h)
    }) {
        if !close_embedded() {
            return Err("请先关闭取色器".into());
        }
        DETACHED.with_borrow_mut(|all| all.retain(|(h, _)| *h != content));
        PAGE.with(|page| page.resume(content, model, bounds));
        return Ok(());
    }
    PAGE.with(|page| page.open(owner, model, bounds, |host| create(host, model, true)))
}

pub(crate) unsafe fn resize_embedded(bounds: RECT) {
    PAGE.with(|page| page.resize(bounds));
}

pub(crate) unsafe fn reload_after_template() {
    PAGE.with(|page| {
        let hwnd = page.content();
        if !hwnd.is_null() {
            let editor = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor);
            let control = editor.visual_selected();
            editor.restore_selection = (control != u16::MAX).then_some(control);
            editor.macro_control = None;
            editor.begin(hwnd, None);
        }
    });
}

pub(crate) unsafe fn embedded_writing() -> bool {
    PAGE.with(|page| page.busy()) || DETACHED.with_borrow(|all| !all.is_empty())
}

pub(crate) unsafe fn close_embedded() -> bool {
    PAGE.with(|page| {
        let hwnd = page.content();
        if !hwnd.is_null() {
            let editor = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor);
            if editor.color_open {
                return false;
            }
            if editor.saving && editor.pending.is_some() {
                let hwnd = page.hide_and_detach();
                DETACHED.with_borrow_mut(|all| all.push((hwnd, editor.model)));
                return true;
            }
        }
        page.close()
    })
}

pub(super) unsafe fn destroyed(hwnd: HWND, data: *mut Editor) {
    if detached(hwnd) {
        DETACHED.with_borrow_mut(|all| all.retain(|(h, _)| *h != hwnd));
        drop(Box::from_raw(data));
    } else {
        PAGE.with(|page| page.destroyed(hwnd, data));
    }
}

pub(super) fn detached(hwnd: HWND) -> bool {
    DETACHED.with_borrow(|all| all.iter().any(|(h, _)| *h == hwnd))
}
pub(crate) unsafe fn writing_model(model: usize) -> bool {
    DETACHED.with_borrow(|all| all.iter().any(|(_, m)| *m == model))
        || PAGE.with(|page| {
            let hwnd = page.content();
            if hwnd.is_null() {
                return false;
            }
            let editor = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor);
            editor.model == model && editor.saving && editor.pending.is_some()
        })
}
pub(crate) fn take_write_notice() -> Option<(usize, String, bool)> {
    NOTICES.with_borrow_mut(|all| all.pop_front())
}
pub(super) unsafe fn notify_write(hwnd: HWND, model: usize, text: String, error: bool) {
    NOTICES.with_borrow_mut(|all| all.push_back((model, text, error)));
    PostMessageW(GetAncestor(hwnd, GA_ROOT), WRITE_FINISHED, 0, 0);
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
        canvas.rect(R::new(0, 0, WIDTH, HEIGHT), palette().bg);
        canvas.text(
            "按键设置",
            R::new(24, 14, 360, 39),
            25,
            600,
            palette().text,
            DT_LEFT,
        );
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
        canvas.text(
            &subtitle,
            R::new(24, 54, 650, 23),
            14,
            400,
            palette().muted,
            DT_LEFT,
        );
        canvas.text(
            "物理按键 / 分层",
            R::new(24, 80, 216, 22),
            14,
            600,
            palette().text,
            DT_LEFT,
        );
        canvas.round(
            R::new(254, 80, 450, 280),
            12,
            palette().panel,
            Some(palette().border),
        );
        let binding = self.binding();
        let title = binding
            .map(|binding| format!("{} · 第 {} 层", binding.label, binding.layer))
            .unwrap_or_else(|| "尚无可用键位".into());
        canvas.text(
            &title,
            R::new(276, 89, 400, 28),
            18,
            600,
            palette().text,
            DT_LEFT,
        );
        canvas.text(
            "操作类型",
            R::new(276, 128, 300, 22),
            13,
            500,
            palette().muted,
            DT_LEFT,
        );
        canvas.text(
            "目标功能 / 按键",
            R::new(276, 198, 372, 22),
            13,
            500,
            palette().muted,
            DT_LEFT,
        );
        if let Some(binding) = binding {
            let access = if self.editable() { "" } else { "（只读）" };
            canvas.text(
                &format!("设备当前设置{access}   {}", self.describe(&binding.action)),
                R::new(276, 312, 408, 36),
                13,
                400,
                palette().muted,
                DT_LEFT | DT_WORDBREAK,
            );
        }
    }
}
