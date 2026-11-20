//! Capability-driven editor for a plugin's active onboard pointer profile.
use super::*;
use crate::paint::palette;
use crate::{devices::DeviceSpec, plugin_manager};
use serde::Deserialize;
use serde_json::{json, Value};

#[path = "razer_pointer.rs"]
mod embedded;
pub(super) use embedded::{
    close_embedded, embedded_writing, open_embedded, reload_after_template, resize_embedded,
};

const FIELDS: usize = 101;
const INPUT: usize = 102;
const CHOICES: usize = 103;
const APPLY: usize = 104;
const RELOAD: usize = 105;
const CLOSE: usize = 106;
const RESULT: u32 = WM_APP + 17;

pub(super) fn available(spec: &DeviceSpec) -> bool {
    [
        "dpi-stages-write",
        "polling-rate-write",
        "pointer-controls",
        "board-name",
    ]
    .iter()
    .any(|cap| spec.supports(cap))
}

#[derive(Deserialize)]
struct Choice {
    value: i32,
    label: String,
}

#[derive(Deserialize)]
struct Control {
    field: String,
    label: String,
    value: i32,
    min: i32,
    max: i32,
    step: i32,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Readout {
    key: String,
    fresh: bool,
    pointer_profile: Option<u8>,
    dpi_min: Option<i32>,
    dpi_max: Option<i32>,
    dpi_step: Option<i32>,
    #[serde(default)]
    dpi_stages: Vec<i32>,
    dpi_stages_y: Option<Vec<i32>>,
    rate: Option<i32>,
    #[serde(default)]
    supported_rates: Vec<i32>,
    #[serde(default)]
    pointer_controls: Vec<Control>,
    #[serde(default)]
    settings_error: String,
}

#[derive(Clone, PartialEq, Eq)]
enum Setting {
    Dpi(&'static str, usize),
    Rate,
    Control(String),
    BoardName,
}
impl Setting {
    fn request(&self, profile: u8, value: impl serde::Serialize) -> Value {
        match self {
            Self::Dpi(axis, stage) => {
                json!({"op":"set-pointer","profile":profile,"field":"dpi","axis":axis,"stage":stage,"value":value})
            }
            Self::Rate => {
                json!({"op":"set-pointer","profile":profile,"field":"rate","value":value})
            }
            Self::Control(field) => {
                json!({"op":"set-pointer","profile":profile,"field":field,"value":value})
            }
            Self::BoardName => json!({"op":"set-board-name","profile":profile,"name":value}),
        }
    }
}

struct Entry {
    setting: Setting,
    title: String,
    current: Option<i32>,
    unit: String,
    input: InputKind,
}
enum InputKind {
    Number { min: i32, max: i32, step: i32 },
    Choices(Vec<Choice>),
    Name(Option<String>),
}
impl Entry {
    fn display_value(&self) -> String {
        if let InputKind::Name(name) = &self.input {
            return name.as_deref().unwrap_or("未上报").to_owned();
        }
        let Some(value) = self.current else {
            return "未上报".into();
        };
        if let InputKind::Choices(options) = &self.input {
            if let Some(choice) = options.iter().find(|choice| choice.value == value) {
                return choice.label.clone();
            }
        }
        format!("{} {}", value, self.unit).trim_end().into()
    }
}
struct Profile {
    index: u8,
    entries: Vec<Entry>,
    note: String,
}
impl Readout {
    fn profile(self, key: &str, dpi: bool, rate: bool, tuning: bool) -> Result<Profile, String> {
        if self.key != key || !self.fresh {
            return Err("设备未响应。唤醒鼠标后可重新读取。".into());
        }
        let index = self.pointer_profile.ok_or_else(|| {
            if self.settings_error.is_empty() {
                "尚未读到板载配置，请重新读取。".to_string()
            } else {
                self.settings_error.clone()
            }
        })?;
        let mut entries = Vec::new();
        if let (true, Some(min), Some(max), Some(step)) =
            (dpi, self.dpi_min, self.dpi_max, self.dpi_step)
        {
            let split = self.dpi_stages_y.is_some();
            for (axis, values) in [
                ("x", self.dpi_stages),
                ("y", self.dpi_stages_y.unwrap_or_default()),
            ] {
                for (stage, value) in values.into_iter().enumerate() {
                    let axis_name = if split && axis == "x" {
                        " · X 轴"
                    } else if axis == "y" {
                        " · Y 轴"
                    } else {
                        ""
                    };
                    entries.push(Entry {
                        setting: Setting::Dpi(axis, stage),
                        title: format!("DPI 档位 {}{}", stage + 1, axis_name),
                        current: Some(value),
                        unit: "DPI".into(),
                        input: InputKind::Number { min, max, step },
                    });
                }
            }
        }
        if rate && !self.supported_rates.is_empty() {
            entries.push(Entry {
                setting: Setting::Rate,
                title: "回报率".into(),
                current: self.rate,
                unit: "Hz".into(),
                input: InputKind::Choices(
                    self.supported_rates
                        .into_iter()
                        .map(|value| Choice {
                            value,
                            label: format!("{value} Hz"),
                        })
                        .collect(),
                ),
            });
        }
        if tuning {
            for control in self.pointer_controls {
                entries.push(Entry {
                    setting: Setting::Control(control.field),
                    title: control.label,
                    current: Some(control.value),
                    unit: control.unit,
                    input: if !control.choices.is_empty() {
                        InputKind::Choices(control.choices)
                    } else {
                        InputKind::Number {
                            min: control.min,
                            max: control.max,
                            step: control.step,
                        }
                    },
                });
            }
        }
        Ok(Profile {
            index,
            entries,
            note: self.settings_error,
        })
    }
}

type EditorRead = Result<(Profile, Box<PointerSample>), String>;

struct Editor {
    spec: &'static DeviceSpec,
    target: Option<crate::hid::DeviceTarget>,
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    profile: Option<Profile>,
    latest: Option<Box<PointerSample>>,
    selected: usize,
    pending: Option<Receiver<EditorRead>>,
    status: String,
    error: bool,
    close_after: bool,
    embedded: bool,
    writing: bool,
}
struct Frame {
    w: i32,
    h: i32,
    panel_x: i32,
    content_h: i32,
}
impl Frame {
    fn new(w: i32, h: i32) -> Self {
        Self {
            w,
            h,
            panel_x: 42 + (w * 32 / 100).clamp(168, 320),
            content_h: (h - 250).max(212),
        }
    }
    fn control(&self, id: usize) -> R {
        match id {
            FIELDS => R::new(24, 120, self.panel_x - 42, self.content_h),
            INPUT | CHOICES => R::new(
                self.panel_x + 22,
                250,
                self.w - self.panel_x - 70,
                if id == INPUT { 35 } else { 240 },
            ),
            RELOAD => R::new(24, self.h - 56, 108, 36),
            CLOSE => R::new(self.w - 256, self.h - 56, 96, 36),
            APPLY => R::new(self.w - 148, self.h - 56, 124, 36),
            _ => unreachable!(),
        }
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
impl Editor {
    unsafe fn frame(&self, hwnd: HWND) -> Frame {
        let mut bounds = RECT::default();
        GetClientRect(hwnd, &mut bounds);
        Frame::new(
            bounds.right * 96 / self.dpi as i32,
            bounds.bottom * 96 / self.dpi as i32,
        )
    }
    unsafe fn resize_controls(&self, hwnd: HWND) {
        let frame = self.frame(hwnd);
        for id in [FIELDS, INPUT, CHOICES, RELOAD, CLOSE, APPLY] {
            let control = GetDlgItem(hwnd, id as i32);
            if !control.is_null() {
                let r = frame.control(id);
                SetWindowPos(
                    control,
                    null_mut(),
                    self.px(r.x),
                    self.px(r.y),
                    self.px(r.w),
                    self.px(r.h),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
        InvalidateRect(hwnd, null(), 0);
    }
    fn entry(&self) -> Option<&Entry> {
        self.profile.as_ref()?.entries.get(self.selected)
    }
    fn px(&self, n: i32) -> i32 {
        n * self.dpi as i32 / 96
    }
    unsafe fn control(&self, hwnd: HWND, id: usize, class: &str, label: &str, rect: R, style: u32) {
        let child = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | paint::native_control_style(class, style),
            self.px(rect.x),
            self.px(rect.y),
            self.px(rect.w),
            self.px(rect.h),
            hwnd,
            id as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(child, WM_SETFONT, self.font as usize, 0);
        if matches!(class, "EDIT" | "LISTBOX" | "COMBOBOX") {
            paint::theme_native_control(child, class, self.dpi);
        }
    }
    unsafe fn begin(&mut self, hwnd: HWND, request: Option<Value>) {
        if self.pending.is_some() {
            return;
        }
        let writing = request.is_some();
        self.writing = writing;
        let spec = self.spec;
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = false;
        self.status = if writing {
            "正在写入并读回设备设置…"
        } else {
            "正在读取板载配置…"
        }
        .into();
        self.enable(hwnd);
        InvalidateRect(hwnd, null(), 0);
        let dest = hwnd as usize;
        let target = self.target.clone();
        thread::spawn(move || {
            let work = plugin_manager::with_device_target(spec.key, target.as_ref(), || {
                if let Some(request) = request {
                    plugin_manager::invoke(spec.key, request)?;
                }
                let read = (|| {
                    let raw = plugin_manager::invoke(
                        spec.key,
                        json!({"op":"read-state","read_pointer":true}),
                    )?;
                    let state = Readout::deserialize(&raw)
                        .map_err(|e| format!("无法识别驱动返回的指针参数：{e}"))?;
                    let mut profile = state.profile(
                        spec.key,
                        spec.supports("dpi-stages-write"),
                        spec.supports("polling-rate-write"),
                        spec.supports("pointer-controls"),
                    )?;
                    if spec.supports("board-name") {
                        let result = plugin_manager::invoke(
                            spec.key,
                            json!({"op":"read-board-name","profile":profile.index}),
                        )
                        .and_then(|reply| {
                            if reply["profile"].as_u64() != Some(u64::from(profile.index)) {
                                return Err("配置编号发生了变化，请重新读取".into());
                            }
                            reply["name"]
                                .as_str()
                                .map(str::to_owned)
                                .ok_or("设备未返回配置名称".into())
                        });
                        let name = match result {
                            Ok(name) => Some(name),
                            Err(error) => {
                                if !profile.note.is_empty() {
                                    profile.note.push(' ');
                                }
                                profile.note.push_str(&format!("读取配置名称失败：{error}"));
                                None
                            }
                        };
                        profile.entries.push(Entry {
                            setting: Setting::BoardName,
                            title: "板载配置名称".into(),
                            current: None,
                            unit: String::new(),
                            input: InputKind::Name(name),
                        });
                    }
                    let sample = serde_json::from_value(raw).map_err(|e| e.to_string())?;
                    Ok::<_, String>((profile, sample))
                })();
                read.map_err(|error| {
                    if writing {
                        format!("驱动已完成写入；刷新配置失败。{error}")
                    } else {
                        error
                    }
                })
            });
            if tx.send(work).is_ok() {
                PostMessageW(dest as HWND, RESULT, 0, 0);
            }
        });
    }
    unsafe fn enable(&self, hwnd: HWND) {
        let ready = self.pending.is_none();
        for id in [FIELDS, INPUT, CHOICES, APPLY] {
            EnableWindow(
                GetDlgItem(hwnd, id as i32),
                i32::from(ready && self.entry().is_some()),
            );
        }
        EnableWindow(GetDlgItem(hwnd, RELOAD as i32), i32::from(ready));
    }
    unsafe fn select(&mut self, hwnd: HWND) {
        let numeric = GetDlgItem(hwnd, INPUT as i32);
        let choices = GetDlgItem(hwnd, CHOICES as i32);
        ShowWindow(numeric, SW_HIDE);
        ShowWindow(choices, SW_HIDE);
        if let Some(entry) = self.entry() {
            match &entry.input {
                InputKind::Number { .. } => {
                    SetWindowTextW(
                        numeric,
                        wide(&entry.current.map(|v| v.to_string()).unwrap_or_default()).as_ptr(),
                    );
                    ShowWindow(numeric, SW_SHOWNA);
                }
                InputKind::Name(name) => {
                    SetWindowTextW(numeric, wide(name.as_deref().unwrap_or_default()).as_ptr());
                    ShowWindow(numeric, SW_SHOWNA);
                }
                InputKind::Choices(options) => {
                    SendMessageW(choices, CB_RESETCONTENT, 0, 0);
                    for (i, option) in options.iter().enumerate() {
                        SendMessageW(
                            choices,
                            CB_ADDSTRING,
                            0,
                            wide(&option.label).as_ptr() as isize,
                        );
                        if Some(option.value) == entry.current {
                            SendMessageW(choices, CB_SETCURSEL, i, 0);
                        }
                    }
                    ShowWindow(choices, SW_SHOWNA);
                }
            }
        }
        self.enable(hwnd);
        InvalidateRect(hwnd, null(), 0);
    }
    unsafe fn apply(&mut self, hwnd: HWND) {
        let Some(entry) = self.entry() else { return };
        let input = GetDlgItem(hwnd, INPUT as i32);
        let input_text = || {
            let mut buffer = vec![0; GetWindowTextLengthW(input) as usize + 1];
            let n = GetWindowTextW(input, buffer.as_mut_ptr(), buffer.len() as i32);
            String::from_utf16_lossy(&buffer[..n as usize])
        };
        let value = match &entry.input {
            InputKind::Name(current) => {
                let name = input_text();
                if current.as_ref() != Some(&name) {
                    if let Some(profile) = &self.profile {
                        self.begin(hwnd, Some(entry.setting.request(profile.index, name)));
                    }
                }
                return;
            }
            InputKind::Number { min, max, step } => {
                input_text().trim().parse::<i32>().ok().filter(|value| {
                    value >= min
                        && value <= max
                        && *step > 0
                        && (i64::from(*value) - i64::from(*min)) % i64::from(*step) == 0
                })
            }
            InputKind::Choices(options) => {
                let i = SendMessageW(GetDlgItem(hwnd, CHOICES as i32), CB_GETCURSEL, 0, 0);
                usize::try_from(i)
                    .ok()
                    .and_then(|i| options.get(i))
                    .map(|o| o.value)
            }
        };
        let Some(value) = value else {
            self.status = "请按上方范围和步长输入，或从可用值中选择。".into();
            self.error = true;
            InvalidateRect(hwnd, null(), 0);
            return;
        };
        if entry.current == Some(value) {
            self.status = "设备当前已使用此值。".into();
            self.error = false;
            InvalidateRect(hwnd, null(), 0);
        } else if let Some(profile) = &self.profile {
            self.begin(hwnd, Some(entry.setting.request(profile.index, value)));
        }
    }
    unsafe fn finish(&mut self, hwnd: HWND) {
        let Some(rx) = self.pending.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("驱动任务已中断。".into()),
        };
        self.pending = None;
        let written = std::mem::take(&mut self.writing);
        let previous = self.entry().map(|entry| entry.setting.clone());
        self.error = result.is_err();
        self.profile = match result {
            Ok((profile, sample)) => {
                self.latest = Some(sample);
                self.status = if profile.entries.is_empty() {
                    "没有收到可编辑的参数。".into()
                } else if written {
                    "已写入设备并重新读取。".into()
                } else {
                    "已读取设备配置。".into()
                };
                if !profile.note.is_empty() {
                    self.status.push(' ');
                    self.status.push_str(&profile.note);
                }
                Some(profile)
            }
            Err(error) => {
                self.latest = None;
                self.status = error;
                None
            }
        };
        let list = GetDlgItem(hwnd, FIELDS as i32);
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        if let Some(profile) = &self.profile {
            for entry in &profile.entries {
                SendMessageW(list, LB_ADDSTRING, 0, wide(&entry.title).as_ptr() as isize);
            }
            self.selected = profile
                .entries
                .iter()
                .position(|entry| Some(&entry.setting) == previous.as_ref())
                .unwrap_or(0);
            SendMessageW(list, LB_SETCURSEL, self.selected, 0);
        }
        self.select(hwnd);
        if self.close_after {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        {
            let c = Canvas::new(dc, self.dpi);
            let f = self.frame(hwnd);
            let field_x = f.panel_x + 22;
            let field_width = f.w - field_x - 46;
            c.rect(R::new(0, 0, f.w, f.h), palette().bg);
            c.text(
                "指针与性能",
                R::new(24, 14, 400, 39),
                25,
                600,
                palette().text,
                DT_LEFT,
            );
            c.text(
                self.spec.name,
                R::new(25, 55, 420, 24),
                14,
                400,
                palette().muted,
                DT_LEFT,
            );
            if let Some(profile) = &self.profile {
                c.text(
                    &format!("板载配置 {}", u16::from(profile.index) + 1),
                    R::new(f.w - 214, 38, 188, 28),
                    13,
                    500,
                    palette().muted,
                    DT_RIGHT,
                );
            }
            c.text(
                "设备参数",
                R::new(24, 90, 200, 26),
                14,
                600,
                palette().text,
                DT_LEFT,
            );
            c.round(
                R::new(f.panel_x, 120, f.w - f.panel_x - 24, f.content_h),
                12,
                palette().panel,
                Some(palette().border),
            );
            if let Some(entry) = self.entry() {
                c.text(
                    &entry.title,
                    R::new(field_x, 137, field_width, 33),
                    19,
                    600,
                    palette().text,
                    DT_LEFT,
                );
                c.text(
                    &format!("设备读值   {}", entry.display_value()),
                    R::new(field_x, 179, field_width, 27),
                    14,
                    400,
                    palette().muted,
                    DT_LEFT,
                );
                c.text(
                    "设置为",
                    R::new(field_x, 220, field_width, 22),
                    13,
                    500,
                    palette().text,
                    DT_LEFT,
                );
                let hint = match &entry.input {
                    InputKind::Number { min, max, step } => {
                        format!("{min}–{max} {}   ·   步长 {step}", entry.unit)
                    }
                    InputKind::Choices(_) | InputKind::Name(_) => String::new(),
                };
                c.text(
                    &hint,
                    R::new(field_x, 301, field_width, 27),
                    12,
                    400,
                    palette().muted,
                    DT_LEFT,
                );
            } else {
                c.text(
                    if self.pending.is_some() {
                        "正在读取…"
                    } else {
                        "暂无可编辑参数"
                    },
                    R::new(field_x, 165, field_width, 55),
                    19,
                    500,
                    palette().muted,
                    DT_LEFT,
                );
            }
            c.text(
                &self.status,
                R::new(25, f.h - 117, f.w - 50, 57),
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
        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
        return DefWindowProcW(hwnd, msg, w, l);
    }
    if msg == WM_NCDESTROY {
        let data = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        embedded::destroyed(hwnd, data as *mut Editor);
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let Some(editor) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor).as_mut() else {
        return DefWindowProcW(hwnd, msg, w, l);
    };
    match msg {
        WM_SIZE => {
            editor.resize_controls(hwnd);
            0
        }
        WM_DPICHANGED_AFTERPARENT if editor.embedded => {
            let next = GetDpiForWindow(hwnd).max(96);
            if next != editor.dpi {
                let old = editor.font;
                editor.dpi = next;
                editor.font = font(next);
                for id in [FIELDS, INPUT, CHOICES, RELOAD, APPLY] {
                    let child = GetDlgItem(hwnd, id as i32);
                    if !child.is_null() {
                        SendMessageW(child, WM_SETFONT, editor.font as usize, 1);
                    }
                }
                DeleteObject(old);
                editor.resize_controls(hwnd);
            }
            0
        }
        WM_PAINT => {
            editor.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            SetTextColor(w as HDC, crate::paint::colorref(palette().text));
            SetBkColor(w as HDC, crate::paint::colorref(palette().field));
            editor.brush as isize
        }
        WM_MEASUREITEM => {
            paint::measure_native_choice(&mut *(l as *mut MEASUREITEMSTRUCT), editor.dpi) as isize
        }
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if paint::draw_native_choice(item, editor.dpi, editor.font) {
                return 1;
            }
            if item.CtlType != ODT_BUTTON {
                return 0;
            }
            let c = Canvas::new(item.hDC, editor.dpi);
            let width = item.rcItem.right * 96 / editor.dpi as i32;
            let height = item.rcItem.bottom * 96 / editor.dpi as i32;
            let disabled = item.itemState & ODS_DISABLED != 0;
            c.rect(R::new(0, 0, width + 1, height + 1), palette().bg);
            c.round(
                R::new(0, 0, width - 1, height - 1),
                8,
                if !disabled && item.CtlID as usize == APPLY {
                    palette().primary
                } else {
                    palette().field
                },
                Some(if item.itemState & ODS_FOCUS != 0 {
                    palette().muted
                } else {
                    palette().border
                }),
            );
            let title = match item.CtlID as usize {
                APPLY => "应用到设备",
                RELOAD => "重新读取",
                _ => "关闭",
            };
            c.text(
                title,
                R::new(3, 0, width - 6, height),
                14,
                500,
                if disabled {
                    palette().dim
                } else if item.CtlID as usize == APPLY {
                    palette().on_primary
                } else {
                    palette().text
                },
                DT_CENTER,
            );
            1
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            let notice = w >> 16 & 0xffff;
            if matches!(id, CLOSE) || id == IDCANCEL as usize {
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
            } else if id == FIELDS && notice == LBN_SELCHANGE as usize {
                let idx = SendMessageW(GetDlgItem(hwnd, FIELDS as i32), LB_GETCURSEL, 0, 0);
                if let Ok(i) = usize::try_from(idx) {
                    editor.selected = i;
                    editor.select(hwnd);
                }
            } else if editor.pending.is_none() && notice == BN_CLICKED as usize {
                match id {
                    APPLY => editor.apply(hwnd),
                    RELOAD => editor.begin(hwnd, None),
                    _ => (),
                }
            }
            0
        }
        RESULT => {
            editor.finish(hwnd);
            0
        }
        WM_CLOSE => {
            if editor.embedded {
                // The main window owns navigation and the child lifetime.
                return 0;
            }
            if editor.pending.is_some() {
                editor.close_after = true;
                editor.status = "当前设备操作完成后将关闭。".into();
                InvalidateRect(hwnd, null(), 0);
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

unsafe fn font(dpi: u32) -> HFONT {
    CreateFontW(
        -((14 * dpi / 96) as i32),
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
    )
}

unsafe fn create(
    owner: HWND,
    spec: &'static DeviceSpec,
    embedded_bounds: Option<RECT>,
) -> Result<(HWND, Box<Editor>), String> {
    let dpi = GetDpiForWindow(owner).max(96);
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskPointerEditor");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    });
    let font = font(dpi);
    let brush = CreateSolidBrush(crate::paint::colorref(palette().field));
    let mut editor = Box::new(Editor {
        spec,
        target: plugin_manager::selected_device_target(spec.key),
        dpi,
        font,
        brush,
        profile: None,
        latest: None,
        selected: 0,
        pending: None,
        status: String::new(),
        error: false,
        close_after: false,
        writing: false,
        embedded: embedded_bounds.is_some(),
    });
    let (style, ex_style, x, y, width, height) = if let Some(r) = embedded_bounds {
        (
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            WS_EX_CONTROLPARENT,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
        )
    } else {
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: editor.px(720),
            bottom: editor.px(482),
        };
        AdjustWindowRectExForDpi(&mut bounds, style, 0, WS_EX_DLGMODALFRAME, dpi);
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let mut owner_rect = zeroed();
        GetWindowRect(owner, &mut owner_rect);
        (
            style,
            WS_EX_DLGMODALFRAME,
            owner_rect.left + ((owner_rect.right - owner_rect.left - width) / 2).max(0),
            owner_rect.top + ((owner_rect.bottom - owner_rect.top - height) / 2).max(0),
            width,
            height,
        )
    };
    let hwnd = CreateWindowExW(
        ex_style,
        class.as_ptr(),
        wide(&format!("{} · 指针与性能", spec.name)).as_ptr(),
        style,
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        instance,
        (&mut *editor as *mut Editor).cast(),
    );
    if hwnd.is_null() {
        return Err(format!(
            "无法打开指针编辑器：{}",
            std::io::Error::last_os_error()
        ));
    }
    if !editor.embedded {
        let dark = i32::from(crate::paint::theme::is_dark());
        DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    }
    let frame = editor.frame(hwnd);
    editor.control(
        hwnd,
        FIELDS,
        "LISTBOX",
        "设备参数",
        frame.control(FIELDS),
        LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL,
    );
    editor.control(
        hwnd,
        INPUT,
        "EDIT",
        "",
        frame.control(INPUT),
        ES_AUTOHSCROLL as u32,
    );
    editor.control(
        hwnd,
        CHOICES,
        "COMBOBOX",
        "",
        frame.control(CHOICES),
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    for (id, title) in [(RELOAD, "重新读取"), (CLOSE, "关闭"), (APPLY, "应用到设备")] {
        if id != CLOSE || !editor.embedded {
            editor.control(
                hwnd,
                id,
                "BUTTON",
                title,
                frame.control(id),
                BS_OWNERDRAW as u32,
            );
        }
    }
    editor.select(hwnd);
    Ok((hwnd, editor))
}

pub(super) unsafe fn open(owner: HWND, spec: &'static DeviceSpec) -> Option<Box<PointerSample>> {
    let (hwnd, mut editor) = match create(owner, spec, None) {
        Ok(window) => window,
        Err(e) => {
            message(&e);
            return None;
        }
    };
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetActiveWindow(hwnd);
    editor.begin(hwnd, None);
    let mut message: MSG = zeroed();
    let mut quit = None;
    while IsWindow(hwnd) != 0 {
        match GetMessageW(&mut message, null_mut(), 0, 0) {
            -1 => break,
            0 => {
                quit = Some(message.wParam as i32);
                break;
            }
            _ => {
                if IsDialogMessageW(hwnd, &message) == 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
    if IsWindow(hwnd) != 0 {
        DestroyWindow(hwnd);
    }
    EnableWindow(owner, 1);
    SetActiveWindow(owner);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
    editor.latest.take()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_layout_uses_the_embedded_viewport_for_controls_and_footer() {
        let modal = Frame::new(720, 482);
        let page = Frame::new(980, 680);
        assert!(page.control(FIELDS).h > modal.control(FIELDS).h);
        assert!(page.control(INPUT).w > modal.control(INPUT).w);
        for frame in [modal, page] {
            let list = frame.control(FIELDS);
            let apply = frame.control(APPLY);
            assert!(list.x + list.w < frame.control(INPUT).x);
            assert_eq!(apply.x + apply.w, frame.w - 24);
            assert_eq!(apply.y + apply.h, frame.h - 20);
        }
    }

    #[test]
    fn editor_targets_reported_profile_axis_stage_and_named_controls() {
        let data = json!({"key":"mouse","fresh":true,"pointer_profile":2,"dpi_min":50,"dpi_max":26000,"dpi_step":50,
            "dpi_stages":[800,1600],"dpi_stages_y":[900,1650],"rate":1000,"supported_rates":[500,1000],
            "pointer_controls":[{"field":"debounce","label":"按键响应","value":2,"min":0,"max":16,"step":1,"unit":"ms"}]});
        let p: Readout = serde_json::from_value(data).unwrap();
        let p = p.profile("mouse", true, true, true).unwrap();
        assert_eq!(p.entries.len(), 6);
        assert_eq!(
            p.entries[3].setting.request(p.index, 1650),
            json!({"op":"set-pointer","profile":2,"field":"dpi","axis":"y","stage":1,"value":1650})
        );
        assert_eq!(p.entries[4].setting.request(p.index, 500)["field"], "rate");
        assert_eq!(
            p.entries[5].setting.request(p.index, 5)["field"],
            "debounce"
        );
        assert_eq!(
            Setting::BoardName.request(p.index, "工作"),
            json!({"op":"set-board-name","profile":2,"name":"工作"})
        );
    }
}
