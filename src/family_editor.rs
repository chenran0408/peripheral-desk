//! Embedded M HUB editors. The installed model plugin supplies the device's pages,
//! keyboard geometry, units and current values and owns every hardware transaction.
mod fields;
#[cfg(test)]
mod tests;
use crate::{
    configuration_ui,
    controls_ui::viewport,
    devices::{DeviceSpec, REGISTRY},
    hid::{wide, DeviceTarget},
    paint::{self, palette, Canvas, R},
    plugin_manager,
};
use fields::*;
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::*, HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::*,
    },
};

// commctrl.h defines TBM_GETPOS as WM_USER.
const TBM_GETPOS: u32 = WM_USER;
const WIDTH: i32 = 792;
const LEFT: i32 = 180;
const READ: usize = 12;
const APPLY: usize = 13;
const PICK: usize = 14;
const SCOPE: usize = 15;
const SECTION: usize = 100;
const KEY: usize = 1024;
const FIELD: usize = 4096;
const SLIDER: usize = 8192;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Family {
    Audio,
    Magnetic,
}
impl Family {
    pub(crate) fn for_device(spec: &DeviceSpec) -> Option<Self> {
        if spec.supports("magnetic-settings") {
            Some(Self::Magnetic)
        } else if spec.supports("mchose-audio-settings") {
            Some(Self::Audio)
        } else {
            None
        }
    }
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Audio => "音频设置",
            Self::Magnetic => "磁轴高级设置",
        }
    }
    fn op(self, write: bool) -> &'static str {
        match (self, write) {
            (Self::Audio, false) => "mhub-audio-read",
            (Self::Audio, true) => "mhub-audio-apply",
            (Self::Magnetic, false) => "mhub-magnetic-read",
            (Self::Magnetic, true) => "mhub-magnetic-apply",
        }
    }
}
struct Editor {
    key: &'static str,
    name: &'static str,
    target: Option<DeviceTarget>,
    family: Family,
    page: DevicePage,
    controls: Vec<HWND>,
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    pending: Option<mpsc::Receiver<Result<DevicePage, String>>>,
    writing: bool,
    building: bool,
    status: String,
    error: bool,
    row_top: i32,
    width: i32,
    height: i32,
}
thread_local! { static PAGE: viewport::Embedded<Editor> = const { viewport::Embedded::new() }; }
impl viewport::EditorState for Editor {
    fn blocks_close(&self) -> bool {
        self.writing
    }
}
impl Drop for Editor {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.brush);
        }
    }
}
pub(crate) unsafe fn open(
    owner: HWND,
    model: usize,
    bounds: RECT,
    family: Family,
) -> Result<(), String> {
    let spec = &REGISTRY[model];
    let target = plugin_manager::selected_device_target(spec.key);
    PAGE.with(|p| {
        let current = p.content();
        if !current.is_null() {
            let state = &*(GetWindowLongPtrW(current, GWLP_USERDATA) as *const Editor);
            if (state.target != target || state.key != spec.key) && !close() {
                return Err("请先完成当前设备的编辑".into());
            }
        }
        p.open(owner, model, bounds, |host| {
            create(
                host,
                spec.key,
                spec.name,
                target,
                family,
                bounds.right - bounds.left,
            )
        })
    })
}
pub(crate) unsafe fn resize(bounds: RECT) {
    PAGE.with(|p| {
        p.resize(bounds);
        let h = p.content();
        if h.is_null() {
            return;
        }
        let e = &mut *(GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Editor);
        let width = page_width(bounds.right - bounds.left, e.dpi);
        if width != e.width {
            let focused = GetDlgCtrlID(windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus());
            let drafts: Vec<_> = e
                .page
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let ctrl = GetDlgItem(h, (FIELD + i) as i32);
                    (
                        configuration_ui::text(ctrl),
                        (!f.choices.is_empty()).then(|| SendMessageW(ctrl, CB_GETCURSEL, 0, 0)),
                    )
                })
                .collect();
            e.width = width;
            e.build(h);
            e.building = true;
            for (i, (text, choice)) in drafts.iter().enumerate() {
                let id = (FIELD + i) as i32;
                if let Some(index) = choice {
                    SendDlgItemMessageW(h, id, CB_SETCURSEL, *index as usize, 0);
                } else {
                    SetDlgItemTextW(h, id, wide(text).as_ptr());
                }
            }
            e.building = false;
            e.sync_sliders(h);
            e.enabled(h);
            if focused > 0 {
                let next = GetDlgItem(h, focused);
                if !next.is_null() {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(next);
                }
            }
        }
    });
}
fn page_width(px: i32, dpi: u32) -> i32 {
    (px * 96 / dpi as i32 - 18).max(WIDTH - 40)
}
pub(crate) unsafe fn writing() -> bool {
    PAGE.with(|p| p.busy())
}
pub(crate) unsafe fn close() -> bool {
    PAGE.with(|p| {
        let hwnd = p.content();
        if !hwnd.is_null() {
            let editor = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor);
            if editor.writing || !editor.discard(hwnd) {
                return false;
            }
        }
        p.close()
    })
}
pub(crate) unsafe fn allow_replacement() -> bool {
    PAGE.with(|p| {
        let hwnd = p.content();
        if hwnd.is_null() {
            return true;
        }
        let editor = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor);
        !editor.writing && editor.discard(hwnd)
    })
}
pub(crate) unsafe fn reload() {
    PAGE.with(|p| {
        let h = p.content();
        if !h.is_null() {
            let editor = &mut *(GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Editor);
            editor.read_current(h);
        }
    })
}
unsafe fn create(
    host: HWND,
    key: &'static str,
    name: &'static str,
    target: Option<DeviceTarget>,
    family: Family,
    width_px: i32,
) -> Result<(HWND, Box<Editor>), String> {
    let module = GetModuleHandleW(null());
    let class = wide("PeripheralDeskFamilyEditor");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as _,
        lpfnWndProc: Some(proc),
        hInstance: module,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    });
    InitCommonControlsEx(&INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_BAR_CLASSES,
    });
    let dpi = GetDpiForWindow(host).max(96);
    let font = CreateFontW(
        -((14 * dpi / 96) as i32),
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as _,
        OUT_DEFAULT_PRECIS as _,
        CLIP_DEFAULT_PRECIS as _,
        CLEARTYPE_QUALITY as _,
        DEFAULT_PITCH as _,
        wide("Microsoft YaHei UI").as_ptr(),
    );
    let mut state = Box::new(Editor {
        key,
        name,
        target,
        family,
        page: DevicePage::default(),
        controls: vec![],
        dpi,
        font,
        brush: CreateSolidBrush(paint::colorref(palette().field)),
        pending: None,
        writing: false,
        building: false,
        status: String::new(),
        error: false,
        row_top: 100,
        width: page_width(width_px, dpi),
        height: 510,
    });
    let h = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class.as_ptr(),
        wide(family.title()).as_ptr(),
        WS_CHILD | WS_CLIPCHILDREN,
        0,
        0,
        state.px(state.width),
        state.px(510),
        host,
        null_mut(),
        module,
        (&mut *state as *mut Editor).cast(),
    );
    if h.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    state.build(h);
    state.read_current(h);
    Ok((h, state))
}
impl Editor {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    unsafe fn control(
        &mut self,
        h: HWND,
        id: usize,
        class: &str,
        title: &str,
        style: u32,
        r: R,
    ) -> HWND {
        let ctrl = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(title).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | paint::native_control_style(class, style),
            self.px(r.x),
            self.px(r.y),
            self.px(r.w),
            self.px(r.h),
            h,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(ctrl, WM_SETFONT, self.font as usize, 0);
        paint::theme_native_control(ctrl, class, self.dpi);
        viewport::follow_focus(ctrl, h);
        self.controls.push(ctrl);
        ctrl
    }
    unsafe fn button(&mut self, h: HWND, id: usize, title: &str, r: R) {
        self.control(h, id, "BUTTON", title, BS_OWNERDRAW as u32, r);
    }
    unsafe fn choice(&mut self, h: HWND, id: usize, items: &[(i32, String)], value: i32, r: R) {
        let ctrl = self.control(
            h,
            id,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
            R::new(r.x, r.y, r.w, 240),
        );
        for (index, (v, label)) in items.iter().enumerate() {
            SendMessageW(ctrl, CB_ADDSTRING, 0, wide(label).as_ptr() as _);
            if *v == value {
                SendMessageW(ctrl, CB_SETCURSEL, index, 0);
            }
        }
    }
    unsafe fn build(&mut self, h: HWND) {
        self.building = true;
        for child in self.controls.drain(..) {
            DestroyWindow(child);
        }
        let names: Vec<String> = self.page.sections.iter().map(|s| s.label.clone()).collect();
        for (i, label) in names.iter().enumerate() {
            self.button(
                h,
                SECTION + i,
                label,
                R::new(16, 82 + i as i32 * 42, 140, 36),
            );
        }
        self.button(h, READ, "重新读取", R::new(self.width - 133, 19, 112, 34));
        let scope_offset = if let Some(scope) = &self.page.scope {
            let opt: Vec<_> = scope
                .choices
                .iter()
                .map(|o| (o.value, o.label.clone()))
                .collect();
            self.choice(
                h,
                SCOPE,
                &opt,
                scope.value,
                R::new(LEFT + 182, 104, 360, 32),
            );
            52
        } else {
            0
        };
        self.row_top = 104 + scope_offset;
        if let Some(sel) = &self.page.selector {
            let geometry = keyboard_geometry(&sel.choices, self.width);
            let choices: Vec<_> = sel
                .choices
                .iter()
                .map(|s| (s.value, s.label.clone()))
                .collect();
            let value = sel.value;
            if let Some(keys) = geometry {
                let bottom = keys.iter().map(|r| r.y + r.h).max().unwrap_or(125);
                for (i, mut r) in keys.into_iter().enumerate() {
                    r.y += scope_offset;
                    self.button(h, KEY + i, &choices[i].1, r);
                }
                self.row_top = bottom + 20 + scope_offset;
            } else {
                self.choice(
                    h,
                    PICK,
                    &choices,
                    value,
                    R::new(LEFT + 182, 114 + scope_offset, 360, 34),
                );
                self.row_top = 166 + scope_offset;
            }
        }
        if !self.page.curve.is_empty() {
            self.row_top += 152;
        }
        for i in 0..self.page.fields.len() {
            let f = &self.page.fields[i];
            let value = f.value;
            let formatted = f.display(value);
            let options: Vec<_> = f
                .choices
                .iter()
                .map(|o| (o.value, o.label.clone()))
                .collect();
            let y = self.row_top + i as i32 * 46 + 6;
            let bounds = f
                .min
                .zip(f.max)
                .filter(|(lo, hi)| !f.readonly && options.is_empty() && lo < hi);
            let step = f.step.unwrap_or(1).max(1);
            let r = if let Some((lo, hi)) = bounds {
                let bar = self.control(
                    h,
                    SLIDER + i,
                    "msctls_trackbar32",
                    "",
                    TBS_NOTICKS,
                    R::new(LEFT + 202, y, self.width - LEFT - 435, 30),
                );
                let num =
                    ((i64::from(hi) - i64::from(lo)) / i64::from(step)).min(i32::MAX as i64) as i32;
                SendMessageW(bar, TBM_SETRANGEMAX, 0, num as isize);
                SendMessageW(
                    bar,
                    TBM_SETPOS,
                    1,
                    ((i64::from(value) - i64::from(lo)) / i64::from(step)) as isize,
                );
                R::new(self.width - 212, y, 106, 32)
            } else {
                R::new(LEFT + 278, y, self.width - LEFT - 350, 32)
            };
            if options.is_empty() {
                self.control(h, FIELD + i, "EDIT", &formatted, ES_AUTOHSCROLL as u32, r);
            } else {
                self.choice(h, FIELD + i, &options, value, r);
            }
        }
        self.height = (self.row_top + self.page.fields.len() as i32 * 46 + 155)
            .max(112 + self.page.sections.len() as i32 * 42)
            .max(478);
        self.button(
            h,
            APPLY,
            "应用设置",
            R::new(self.width - 150, self.height - 65, 128, 38),
        );
        viewport::content_size(h, self.px(self.width), self.px(self.height));
        self.building = false;
        self.enabled(h);
        InvalidateRect(h, null(), 0);
    }
    unsafe fn enabled(&self, h: HWND) {
        let idle = self.pending.is_none();
        for &child in &self.controls {
            let id = GetDlgCtrlID(child) as usize;
            let available = match id {
                APPLY => self.changes(h).is_ok_and(|v| !v.is_empty()),
                n if n >= SLIDER => true,
                n if n >= FIELD => self.page.fields.get(n - FIELD).is_some_and(|f| !f.readonly),
                _ => true,
            };
            EnableWindow(child, i32::from(idle && available));
        }
    }
    unsafe fn changes(&self, h: HWND) -> Result<Changes, String> {
        let mut changes = Changes::new();
        for (i, f) in self
            .page
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.readonly)
        {
            let child = GetDlgItem(h, (FIELD + i) as i32);
            let val = if f.choices.is_empty() {
                let text = crate::configuration_ui::text(child);
                if text == f.display(f.value) {
                    continue;
                }
                f.parse(&text)?
            } else {
                let index = SendMessageW(child, CB_GETCURSEL, 0, 0);
                let Some(choice) = f.choices.get(index as usize) else {
                    continue;
                };
                choice.value
            };
            if val != f.value {
                changes.insert(f.id.clone(), val);
            }
        }
        Ok(changes)
    }
    unsafe fn discard(&self, h: HWND) -> bool {
        self.changes(h).is_ok_and(|c| c.is_empty())
            || MessageBoxW(
                h,
                wide("放弃尚未应用的修改？").as_ptr(),
                wide(self.family.title()).as_ptr(),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
            ) == IDYES
    }
    unsafe fn read_current(&mut self, h: HWND) {
        self.start(
            h,
            self.page.section.clone(),
            self.page.selector.as_ref().map(|s| s.value),
            None,
        );
    }
    unsafe fn start(
        &mut self,
        h: HWND,
        section: String,
        selector: Option<i32>,
        changes: Option<Changes>,
    ) {
        let scope = (section == self.page.section)
            .then(|| self.page.scope.as_ref().map(|s| s.value))
            .flatten();
        self.start_scoped(h, section, selector, scope, changes);
    }
    unsafe fn start_scoped(
        &mut self,
        h: HWND,
        section: String,
        selector: Option<i32>,
        scope: Option<i32>,
        changes: Option<Changes>,
    ) {
        if self.pending.is_some() {
            return;
        }
        if self.target.is_none() {
            self.target = plugin_manager::selected_device_target(self.key);
        }
        let Some(target) = self.target.clone() else {
            self.status = "设备未连接，连接后可重新读取".into();
            self.error = true;
            InvalidateRect(h, null(), 0);
            return;
        };
        let key = self.key;
        self.writing = changes.is_some();
        let op = self.family.op(self.writing);
        let mut req =
            serde_json::json!({"op":op,"section":section,"selector":selector,"scope":scope});
        if let Some(map) = changes {
            req["changes"] = serde_json::json!(map);
            if let Some(slot) = self.page.profile {
                req["profile"] = slot.into();
            }
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = false;
        self.status = if self.writing {
            "正在应用设置…"
        } else {
            "正在读取设备…"
        }
        .into();
        self.enabled(h);
        InvalidateRect(h, null(), 0);
        SetTimer(h, 1, 50, None);
        std::thread::spawn(move || {
            let result = plugin_manager::invoke_target(key, req, &target).and_then(|v| {
                serde_json::from_value(v).map_err(|e| format!("设备设置页数据无效：{e}"))
            });
            let _ = tx.send(result);
        });
    }
    unsafe fn complete(&mut self, h: HWND) {
        let Some(rx) = &self.pending else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("设备任务已结束，未返回结果".into()),
        };
        KillTimer(h, 1);
        self.pending = None;
        let wrote = std::mem::take(&mut self.writing);
        match result {
            Ok(page) => {
                self.page = page;
                self.status = if wrote {
                    "设置已应用并读回".into()
                } else {
                    String::new()
                };
                self.error = false;
                self.build(h);
            }
            Err(e) => {
                self.error = true;
                self.status = e;
                self.enabled(h);
                InvalidateRect(h, null(), 0);
            }
        }
    }
    unsafe fn sync_slider(&self, h: HWND, i: usize) {
        let f = &self.page.fields[i];
        let bar = GetDlgItem(h, (SLIDER + i) as i32);
        if !bar.is_null() {
            if let Ok(raw) = f.parse(&configuration_ui::text(GetDlgItem(h, (FIELD + i) as i32))) {
                SendMessageW(
                    bar,
                    TBM_SETPOS,
                    1,
                    ((i64::from(raw) - i64::from(f.min.unwrap_or(0)))
                        / i64::from(f.step.unwrap_or(1).max(1))) as isize,
                );
            }
        }
    }
    unsafe fn sync_sliders(&self, h: HWND) {
        for i in 0..self.page.fields.len() {
            self.sync_slider(h, i);
        }
    }
    unsafe fn slider(&mut self, h: HWND, child: HWND) {
        if child.is_null() || self.pending.is_some() || self.building {
            return;
        }
        let Some(i) = (GetDlgCtrlID(child) as usize).checked_sub(SLIDER) else {
            return;
        };
        if let Some(f) = self.page.fields.get(i) {
            let pos = SendMessageW(child, TBM_GETPOS, 0, 0) as i64;
            let raw = i64::from(f.min.unwrap_or(0)) + pos * i64::from(f.step.unwrap_or(1).max(1));
            SetDlgItemTextW(h, (FIELD + i) as i32, wide(&f.display(raw as i32)).as_ptr());
        }
    }
    fn curve_bounds(&self) -> R {
        R::new(LEFT + 30, self.row_top - 143, self.width - LEFT - 64, 112)
    }
    unsafe fn curve_click(&mut self, h: HWND, x: i32, y: i32) {
        let r = self.curve_bounds();
        if self.pending.is_some() || self.page.curve.is_empty() || !r.contains(x, y) {
            return;
        }
        let i =
            ((x - r.x) as f32 / r.w as f32 * (self.page.curve.len() - 1) as f32).round() as usize;
        if let Some(point) = self.page.curve.get(i) {
            let next = point.value;
            if self.page.selector.as_ref().is_some_and(|s| s.value != next) && self.discard(h) {
                self.start(h, self.page.section.clone(), Some(next), None);
            }
        }
    }
    unsafe fn command(&mut self, h: HWND, id: usize, code: u32) {
        if self.building || self.pending.is_some() {
            return;
        }
        let section = self.page.section.clone();
        let selector = self.page.selector.as_ref().map(|s| s.value);
        match id {
            READ if self.discard(h) => self.read_current(h),
            APPLY => match self.changes(h) {
                Ok(c) if !c.is_empty() => self.start(h, section, selector, Some(c)),
                Err(e) => {
                    self.status = e;
                    self.error = true;
                    InvalidateRect(h, null(), 0);
                }
                _ => {}
            },
            SCOPE if code == CBN_SELCHANGE => {
                if let Some(s) = &self.page.scope {
                    let ctrl = GetDlgItem(h, SCOPE as i32);
                    let idx = SendMessageW(ctrl, CB_GETCURSEL, 0, 0) as usize;
                    let scope = s.choices.get(idx).map(|v| v.value);
                    if let Some(old) = s.choices.iter().position(|c| c.value == s.value) {
                        SendMessageW(ctrl, CB_SETCURSEL, old, 0);
                    }
                    if scope != Some(s.value) && self.discard(h) {
                        self.start_scoped(h, section, selector, scope, None);
                    }
                }
            }
            PICK if code == CBN_SELCHANGE => {
                if let Some(s) = &self.page.selector {
                    let ctrl = GetDlgItem(h, PICK as i32);
                    let idx = SendMessageW(ctrl, CB_GETCURSEL, 0, 0) as usize;
                    let value = s.choices.get(idx).map(|v| v.value);
                    if let Some(old) = s.choices.iter().position(|c| c.value == s.value) {
                        SendMessageW(ctrl, CB_SETCURSEL, old, 0);
                    }
                    if value != selector && self.discard(h) {
                        self.start(h, section, value, None);
                    }
                }
            }
            SECTION..KEY if code == BN_CLICKED => {
                if let Some(s) = self.page.sections.get(id - SECTION) {
                    if s.id != section && self.discard(h) {
                        self.start(h, s.id.clone(), None, None);
                    }
                }
            }
            KEY..FIELD if code == BN_CLICKED => {
                let value = self
                    .page
                    .selector
                    .as_ref()
                    .and_then(|s| s.choices.get(id - KEY))
                    .map(|c| c.value);
                if value != selector && self.discard(h) {
                    self.start(h, section, value, None);
                }
            }
            FIELD..SLIDER if code == EN_CHANGE || code == CBN_SELCHANGE => {
                self.sync_slider(h, id - FIELD);
                match self.changes(h) {
                    Ok(c) => {
                        self.error = false;
                        self.status = if c.is_empty() {
                            String::new()
                        } else {
                            "有尚未应用的修改".into()
                        };
                    }
                    Err(e) => {
                        self.error = true;
                        self.status = e;
                    }
                }
                self.enabled(h);
                InvalidateRect(h, null(), 0);
            }
            _ => {}
        }
    }
    unsafe fn paint(&self, h: HWND) {
        let mut ps = zeroed();
        let dc = BeginPaint(h, &mut ps);
        let c = Canvas::new(dc, self.dpi);
        c.rect(R::new(0, 0, self.width, self.height), palette().bg);
        c.text(
            self.name,
            R::new(16, 14, self.width - 164, 43),
            23,
            600,
            palette().text,
            DT_LEFT,
        );
        c.round(
            R::new(6, 68, 160, self.height - 84),
            12,
            palette().panel,
            None,
        );
        c.round(
            R::new(LEFT - 5, 68, self.width - LEFT - 7, self.height - 84),
            12,
            palette().panel,
            None,
        );
        c.text(
            self.page.title(),
            R::new(LEFT + 14, 68, self.width - LEFT - 40, 32),
            17,
            600,
            palette().text,
            DT_LEFT,
        );
        let offset = if let Some(scope) = &self.page.scope {
            c.text(
                &scope.label,
                R::new(LEFT + 14, 104, 155, 32),
                13,
                500,
                palette().muted,
                DT_LEFT,
            );
            52
        } else {
            0
        };
        if let Some(sel) = &self.page.selector {
            c.text(
                &sel.label,
                R::new(LEFT + 14, 102 + offset, 163, 28),
                13,
                500,
                palette().muted,
                DT_LEFT,
            );
        }
        if !self.page.curve.is_empty() {
            let r = self.curve_bounds();
            let mid = r.y + r.h / 2;
            c.line(r.x, mid, r.x + r.w, mid, palette().border, 1.);
            let limit = self
                .page
                .curve
                .iter()
                .map(|p| p.db.abs())
                .fold(12.0_f32, f32::max);
            let mut last = None;
            for (i, point) in self.page.curve.iter().enumerate() {
                let x = r.x + (i as i32 * r.w) / (self.page.curve.len() as i32 - 1).max(1);
                let y = mid - (point.db / limit * (r.h / 2 - 6) as f32).round() as i32;
                if let Some((a, b)) = last {
                    c.line(a, b, x, y, palette().accent, 2.0);
                }
                let size = if self
                    .page
                    .selector
                    .as_ref()
                    .is_some_and(|sel| sel.value == point.value)
                {
                    12
                } else {
                    7
                };
                c.ellipse(
                    R::new(x - size / 2, y - size / 2, size, size),
                    palette().accent,
                );
                c.text(
                    &point.label,
                    R::new(x - 24, r.y + r.h, 48, 23),
                    11,
                    400,
                    palette().muted,
                    DT_CENTER,
                );
                last = Some((x, y));
            }
        }
        for (i, f) in self.page.fields.iter().enumerate() {
            let y = self.row_top + i as i32 * 46;
            c.text(
                &f.label,
                R::new(LEFT + 14, y + 3, 254, 37),
                14,
                500,
                palette().text,
                DT_LEFT,
            );
            c.text(
                &f.unit,
                R::new(self.width - 98, y + 4, 84, 34),
                13,
                400,
                palette().muted,
                DT_LEFT,
            );
        }
        c.text(
            &self.page.note,
            R::new(LEFT + 14, self.height - 145, self.width - LEFT - 36, 65),
            13,
            400,
            palette().muted,
            DT_LEFT | DT_WORDBREAK,
        );
        c.text(
            &self.status,
            R::new(LEFT + 14, self.height - 78, self.width - LEFT - 184, 60),
            13,
            500,
            if self.error {
                palette().error
            } else {
                palette().muted
            },
            DT_LEFT | DT_WORDBREAK,
        );
        drop(c);
        EndPaint(h, &ps);
    }
}

fn keyboard_geometry(choices: &[Choice], width: i32) -> Option<Vec<R>> {
    if choices.is_empty() || choices.iter().any(|c| c.x.is_none() || c.y.is_none()) {
        return None;
    }
    let x0 = choices.iter().filter_map(|c| c.x).reduce(f32::min)?;
    let y0 = choices.iter().filter_map(|c| c.y).reduce(f32::min)?;
    let x1 = choices
        .iter()
        .map(|c| c.x.unwrap() + c.w.unwrap_or(1.))
        .reduce(f32::max)?;
    let y1 = choices
        .iter()
        .map(|c| c.y.unwrap() + c.h.unwrap_or(1.))
        .reduce(f32::max)?;
    let unit = ((width - LEFT - 40) as f32 / (x1 - x0)).min(195.0 / (y1 - y0));
    if !unit.is_finite() || unit <= 0. {
        return None;
    }
    Some(
        choices
            .iter()
            .map(|c| {
                R::new(
                    LEFT + 14 + ((c.x.unwrap() - x0) * unit).round() as i32,
                    132 + ((c.y.unwrap() - y0) * unit).round() as i32,
                    ((c.w.unwrap_or(1.) * unit).round() as i32 - 2).max(1),
                    ((c.h.unwrap_or(1.) * unit).round() as i32 - 2).max(1),
                )
            })
            .collect(),
    )
}

unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            h,
            GWLP_USERDATA,
            (*(l as *const CREATESTRUCTW)).lpCreateParams as _,
        );
    }
    let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Editor;
    if ptr.is_null() {
        return DefWindowProcW(h, msg, w, l);
    }
    let e = &mut *ptr;
    match msg {
        WM_LBUTTONUP => {
            let x = (l as i16) as i32 * 96 / e.dpi as i32;
            let y = ((l >> 16) as i16) as i32 * 96 / e.dpi as i32;
            e.curve_click(h, x, y);
            0
        }
        WM_HSCROLL => {
            e.slider(h, l as HWND);
            0
        }
        WM_COMMAND => {
            e.command(h, w & 0xffff, (w >> 16) as u32);
            0
        }
        WM_TIMER => {
            e.complete(h);
            0
        }
        WM_PAINT => {
            e.paint(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if paint::draw_native_choice(item, e.dpi, e.font) {
                return 1;
            }
            let id = item.CtlID as usize;
            let selected =
                match id {
                    SECTION..KEY => e
                        .page
                        .sections
                        .get(id - SECTION)
                        .is_some_and(|s| s.id == e.page.section),
                    KEY..FIELD => e.page.selector.as_ref().is_some_and(|s| {
                        s.choices.get(id - KEY).is_some_and(|c| c.value == s.value)
                    }),
                    _ => false,
                };
            if (KEY..FIELD).contains(&id) {
                let c = Canvas::new(item.hDC, e.dpi);
                let r = R::new(
                    0,
                    0,
                    item.rcItem.right * 96 / e.dpi as i32,
                    item.rcItem.bottom * 96 / e.dpi as i32,
                );
                let focus = item.itemState & ODS_FOCUS != 0;
                c.rect(r, palette().panel);
                c.round(
                    r,
                    4,
                    if selected {
                        palette().primary
                    } else {
                        palette().field
                    },
                    Some(if focus {
                        palette().accent
                    } else {
                        palette().border
                    }),
                );
                c.text(
                    &configuration_ui::text(item.hwndItem),
                    r,
                    11,
                    500,
                    if selected {
                        palette().on_primary
                    } else {
                        palette().text
                    },
                    DT_CENTER,
                );
                1
            } else {
                configuration_ui::draw_action_button_on(
                    item,
                    e.dpi,
                    if selected { id } else { APPLY },
                    if id == READ {
                        palette().bg
                    } else {
                        palette().panel
                    },
                )
            }
        }
        WM_MEASUREITEM => i32::from(paint::measure_native_choice(
            &mut *(l as *mut MEASUREITEMSTRUCT),
            e.dpi,
        )) as _,
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            SetTextColor(w as HDC, paint::colorref(palette().text));
            SetBkColor(w as HDC, paint::colorref(palette().field));
            e.brush as _
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            PAGE.with(|p| p.destroyed(h, ptr));
            DefWindowProcW(h, msg, w, l)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}
