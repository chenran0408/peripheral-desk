//! Native structured control editor. The device adapters own all wire formats.
use crate::{
    controls::{
        self, BindingAction, ControlAction, ControlState, KeyColor, MacroDefinition, MacroEvent,
        MacroMode,
    },
    devices::REGISTRY,
    hid::wide,
    paint::{self, Canvas, BG, BORDER, DIM, FIELD, MUTED, ORANGE, PANEL, R, RED, TEXT},
};
use std::{
    collections::BTreeMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::LibraryLoader::*,
    UI::{
        Controls::{Dialogs::*, *},
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

const FINISHED: u32 = WM_APP + 80;
const KEYS: i32 = 100;
const KIND: i32 = 101;
const TARGET: i32 = 102;
const MODS: [i32; 4] = [103, 104, 105, 106];
const APPLY: i32 = 110;
const RELOAD: i32 = 111;
const NAME: i32 = 120;
const STEPS: i32 = 121;
const EVENT: i32 = 122;
const EVENT_KEY: i32 = 123;
const DELAY: i32 = 124;
const ADD: i32 = 125;
const DELETE: i32 = 126;
const UP: i32 = 127;
const DOWN: i32 = 128;
const PICK: i32 = 130;
const ALL: i32 = 131;
const STATUS: i32 = 140;
const WIDTH: i32 = 1040;
const HEIGHT: i32 = 750;
type Job = std::result::Result<ControlState, String>;

struct Child {
    hwnd: HWND,
    id: i32,
    text: String,
}
struct Editor {
    model: usize,
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    page: usize,
    children: Vec<Child>,
    state: Option<ControlState>,
    selected: usize,
    pending: Option<Receiver<Job>>,
    saving: bool,
    status: String,
    error: bool,
    keys: Vec<(u16, String)>,
    events: Vec<MacroEvent>,
    macro_name: String,
    macro_control: Option<u16>,
    macro_drafts: BTreeMap<u16, MacroDefinition>,
    rgb: [u8; 3],
    picked: bool,
    color_open: bool,
}
impl Drop for Editor {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.brush);
        }
    }
}
fn colorref(argb: u32) -> u32 {
    ((argb >> 16) & 255) | (argb & 0xff00) | ((argb & 255) << 16)
}
fn key_names() -> Vec<(u16, String)> {
    let mut keys = Vec::new();
    for i in 0..26 {
        keys.push((4 + i, ((b'A' + i as u8) as char).to_string()));
    }
    for (i, name) in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
        .iter()
        .enumerate()
    {
        keys.push((30 + i as u16, (*name).into()));
    }
    for (usage, name) in [
        (40, "回车"),
        (41, "Esc"),
        (42, "退格"),
        (43, "Tab"),
        (44, "空格"),
        (45, "-"),
        (46, "="),
        (47, "["),
        (48, "]"),
        (49, "\\"),
        (51, ";"),
        (52, "'"),
        (53, "`"),
        (54, ","),
        (55, "."),
        (56, "/"),
        (57, "大写锁定"),
    ] {
        keys.push((usage, name.into()));
    }
    for i in 0..12 {
        keys.push((58 + i, format!("F{}", i + 1)));
    }
    for i in 0..12 {
        keys.push((104 + i, format!("F{}", i + 13)));
    }
    for (usage, name) in [
        (70, "截屏"),
        (71, "滚动锁定"),
        (72, "暂停"),
        (73, "Insert"),
        (74, "Home"),
        (75, "Page Up"),
        (76, "Delete"),
        (77, "End"),
        (78, "Page Down"),
        (79, "→ 右方向"),
        (80, "← 左方向"),
        (81, "↓ 下方向"),
        (82, "↑ 上方向"),
        (83, "数字锁定"),
        (84, "小键盘 /"),
        (85, "小键盘 *"),
        (86, "小键盘 -"),
        (87, "小键盘 +"),
        (88, "小键盘回车"),
        (98, "小键盘 0"),
        (99, "小键盘 ."),
        (101, "菜单"),
        (224, "左 Ctrl"),
        (225, "左 Shift"),
        (226, "左 Alt"),
        (227, "左 Win"),
        (228, "右 Ctrl"),
        (229, "右 Shift"),
        (230, "右 Alt"),
        (231, "右 Win"),
    ] {
        keys.push((usage, name.into()));
    }
    for i in 0..9 {
        keys.push((89 + i, format!("小键盘 {}", i + 1)));
    }
    keys
}
const MEDIA: [(u16, &str); 7] = [
    (0xcd, "播放 / 暂停"),
    (0xb5, "下一首"),
    (0xb6, "上一首"),
    (0xb7, "停止"),
    (0xe9, "音量 +"),
    (0xea, "音量 -"),
    (0xe2, "静音"),
];
fn mouse_name(value: u8) -> &'static str {
    match value {
        1 => "左键",
        2 => "右键",
        3 => "中键",
        4 => "后退",
        5 => "前进",
        _ => "未知鼠标功能",
    }
}
impl Editor {
    fn kinds(&self) -> Vec<usize> {
        let declared = self
            .state
            .as_ref()
            .map(|s| s.binding_types.as_slice())
            .unwrap_or(&[]);
        if !declared.is_empty() {
            return ["mouse", "keyboard", "consumer", "disabled", "dpi-cycle"]
                .iter()
                .enumerate()
                .filter_map(|(index, name)| declared.iter().any(|d| d == name).then_some(index))
                .collect();
        }
        match REGISTRY[self.model].key {
            "razer-deathadder-v3-pro" => vec![0, 1, 3],
            "mchose-a5-ultra" => vec![0, 1, 2, 3, 4],
            "mchose-g75-pro" => vec![0, 1, 2, 3],
            _ => vec![],
        }
    }
    unsafe fn kind(&self) -> usize {
        self.kinds()
            .get(self.select(KIND))
            .copied()
            .unwrap_or(usize::MAX)
    }

    fn px(&self, v: i32) -> i32 {
        (v as i64 * self.dpi as i64 / 96) as i32
    }
    fn child(&self, id: i32) -> HWND {
        self.children
            .iter()
            .find(|c| c.id == id)
            .map_or(null_mut(), |c| c.hwnd)
    }
    unsafe fn text(&self, id: i32) -> String {
        let hwnd = self.child(id);
        let n = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut b = vec![0; n + 1];
        GetWindowTextW(hwnd, b.as_mut_ptr(), b.len() as i32);
        String::from_utf16_lossy(&b[..n])
    }
    unsafe fn select(&self, id: i32) -> usize {
        SendMessageW(
            self.child(id),
            if id == KEYS || id == STEPS {
                LB_GETCURSEL
            } else {
                CB_GETCURSEL
            },
            0,
            0,
        ) as usize
    }
    unsafe fn set_select(&self, id: i32, index: usize) {
        SendMessageW(
            self.child(id),
            if id == KEYS || id == STEPS {
                LB_SETCURSEL
            } else {
                CB_SETCURSEL
            },
            index,
            0,
        );
    }
    unsafe fn set_text(&self, id: i32, text: &str) {
        SetWindowTextW(self.child(id), wide(text).as_ptr());
    }
    unsafe fn enable(&self, id: i32, value: bool) {
        EnableWindow(self.child(id), value as i32);
    }
    unsafe fn add(&mut self, hwnd: HWND, id: i32, class: &str, text: &str, r: R, style: u32) {
        let h = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_TABSTOP | paint::native_control_style(class, style),
            self.px(r.x),
            self.px(r.y),
            self.px(r.w),
            self.px(r.h),
            hwnd,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(h, WM_SETFONT, self.font as usize, 0);
        if matches!(class, "EDIT" | "LISTBOX" | "COMBOBOX") {
            paint::theme_native_control(h, class, self.dpi);
        }
        self.children.push(Child {
            hwnd: h,
            id,
            text: text.into(),
        });
    }
    unsafe fn button(&mut self, hwnd: HWND, id: i32, text: &str, r: R) {
        self.add(hwnd, id, "BUTTON", text, r, BS_OWNERDRAW as u32);
    }
    unsafe fn combo(&mut self, hwnd: HWND, id: i32, r: R, options: &[String]) {
        self.add(
            hwnd,
            id,
            "COMBOBOX",
            "",
            r,
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        );
        for option in options {
            SendMessageW(
                self.child(id),
                CB_ADDSTRING,
                0,
                wide(option).as_ptr() as isize,
            );
        }
    }
    fn binding(&self) -> Option<&crate::controls::Binding> {
        self.state.as_ref()?.bindings.get(self.selected)
    }
    fn editable(&self) -> bool {
        self.binding().is_some_and(|b| {
            b.editable
                && match b.action {
                    BindingAction::Unknown(_) => false,
                    BindingAction::Macro(_) => {
                        self.page == 1
                            && self
                                .state
                                .as_ref()
                                .is_some_and(|s| s.macros.contains_key(&b.control))
                    }
                    _ => true,
                }
        })
    }
    fn key_name(&self, usage: u16) -> String {
        self.keys
            .iter()
            .find(|(u, _)| *u == usage)
            .map_or_else(|| format!("未知 HID 0x{usage:02X}"), |(_, s)| s.clone())
    }
    fn describe(&self, action: &BindingAction) -> String {
        match action {
            BindingAction::Disabled => "禁用".into(),
            BindingAction::Mouse(n) => mouse_name(*n).into(),
            BindingAction::Keyboard { usage, modifiers } => {
                let mut parts = Vec::new();
                for (mask, name) in [
                    (0x11, "Ctrl"),
                    (0x22, "Shift"),
                    (0x44, "Alt"),
                    (0x88, "Win"),
                ] {
                    if modifiers & mask != 0 {
                        parts.push(name.to_string());
                    }
                }
                if *usage != 0 {
                    parts.push(self.key_name(*usage));
                }
                parts.join(" + ")
            }
            BindingAction::Consumer(n) => MEDIA
                .iter()
                .find(|(u, _)| u == n)
                .map_or_else(|| format!("媒体 0x{n:04X}"), |(_, s)| (*s).into()),
            BindingAction::DpiCycle => "DPI 循环".into(),
            BindingAction::Macro(n) => format!("设备宏 {n}"),
            BindingAction::Unknown(_) => "未识别（只读）".into(),
        }
    }
    unsafe fn layout(&mut self, hwnd: HWND) {
        self.remember_macro();
        for child in self.children.drain(..) {
            DestroyWindow(child.hwnd);
        }
        for (i, label) in ["按键设置", "宏编辑器", "逐键灯光"].iter().enumerate() {
            self.button(
                hwnd,
                10 + i as i32,
                label,
                R::new(28 + i as i32 * 148, 88, 136, 38),
            );
        }
        self.button(hwnd, RELOAD, "重新读取", R::new(866, 88, 146, 38));
        self.add(
            hwnd,
            KEYS,
            "LISTBOX",
            "",
            R::new(40, 194, 294, 430),
            LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL | WS_HSCROLL,
        );
        SendMessageW(self.child(KEYS), LB_SETITEMHEIGHT, 0, self.px(26) as isize);
        SendMessageW(
            self.child(KEYS),
            LB_SETHORIZONTALEXTENT,
            self.px(600) as usize,
            0,
        );
        let entries: Vec<String> = self
            .state
            .as_ref()
            .map(|s| {
                if self.page == 2 {
                    s.colors
                        .iter()
                        .map(|color| {
                            let label = s
                                .bindings
                                .iter()
                                .find(|b| b.control == color.control && b.layer == 0)
                                .map(|b| b.label.clone())
                                .unwrap_or_else(|| format!("设备键 {}", color.control));
                            format!(
                                "{label}   #{:02X}{:02X}{:02X}",
                                color.rgb[0], color.rgb[1], color.rgb[2]
                            )
                        })
                        .collect()
                } else {
                    s.bindings
                        .iter()
                        .map(|b| {
                            format!("{} · {}层   {}", b.label, b.layer, self.describe(&b.action))
                        })
                        .collect()
                }
            })
            .unwrap_or_default();
        for entry in &entries {
            SendMessageW(
                self.child(KEYS),
                LB_ADDSTRING,
                0,
                wide(entry).as_ptr() as isize,
            );
        }
        if !entries.is_empty() {
            self.selected = self.selected.min(entries.len() - 1);
            self.set_select(KEYS, self.selected);
        }
        match self.page {
            0 => {
                self.combo(
                    hwnd,
                    KIND,
                    R::new(390, 260, 286, 250),
                    &self
                        .kinds()
                        .iter()
                        .map(|i| {
                            [
                                "鼠标功能",
                                "键盘按键 / 组合键",
                                "多媒体",
                                "禁用此键",
                                "DPI 循环",
                            ][*i]
                                .to_string()
                        })
                        .collect::<Vec<_>>(),
                );
                self.combo(hwnd, TARGET, R::new(390, 344, 414, 300), &[]);
                for (i, label) in ["Ctrl", "Shift", "Alt", "Win"].iter().enumerate() {
                    self.add(
                        hwnd,
                        MODS[i],
                        "BUTTON",
                        label,
                        R::new(390 + i as i32 * 102, 394, 94, 28),
                        BS_AUTOCHECKBOX as u32,
                    );
                }
                self.button(hwnd, APPLY, "应用到此键", R::new(838, 592, 150, 38));
                self.sync_binding();
            }
            1 => {
                let name = self.macro_name.clone();
                self.add(
                    hwnd,
                    NAME,
                    "EDIT",
                    &name,
                    R::new(390, 208, 292, 30),
                    ES_AUTOHSCROLL as u32,
                );
                SendMessageW(self.child(NAME), EM_SETLIMITTEXT, 20, 0);
                self.add(
                    hwnd,
                    STEPS,
                    "LISTBOX",
                    "",
                    R::new(390, 260, 430, 212),
                    LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL,
                );
                self.button(hwnd, DELETE, "删除步骤", R::new(840, 260, 148, 36));
                self.button(hwnd, UP, "上移", R::new(840, 308, 148, 36));
                self.button(hwnd, DOWN, "下移", R::new(840, 356, 148, 36));
                self.combo(
                    hwnd,
                    EVENT,
                    R::new(390, 526, 130, 200),
                    &["按键按下", "按键释放", "等待"].map(String::from),
                );
                self.set_select(EVENT, 0);
                let names = self.keys.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>();
                self.combo(hwnd, EVENT_KEY, R::new(532, 526, 176, 300), &names);
                self.set_select(EVENT_KEY, 0);
                self.add(
                    hwnd,
                    DELAY,
                    "EDIT",
                    "100",
                    R::new(720, 526, 94, 30),
                    ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                );
                SendMessageW(self.child(DELAY), EM_SETLIMITTEXT, 5, 0);
                self.button(hwnd, ADD, "添加步骤", R::new(840, 526, 148, 36));
                self.button(hwnd, APPLY, "写入单次宏", R::new(838, 592, 150, 38));
                self.sync_macro();
                self.availability();
            }
            _ => {
                self.button(hwnd, PICK, "选择颜色…", R::new(390, 350, 188, 38));
                self.button(hwnd, APPLY, "应用到选中键", R::new(664, 592, 154, 38));
                self.button(hwnd, ALL, "应用到全部键", R::new(834, 592, 154, 38));
                self.sync_color();
            }
        }
        self.add(
            hwnd,
            STATUS,
            "EDIT",
            &self.status.clone(),
            R::new(28, 678, 984, 52),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        for child in &self.children {
            ShowWindow(child.hwnd, SW_SHOWNA);
        }
        self.availability();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn availability(&self) {
        let ready = self.pending.is_none();
        self.enable(RELOAD, ready);
        self.enable(KEYS, ready);
        for id in 10..13 {
            self.enable(id, ready);
        }
        let editable = ready && self.editable();
        match self.page {
            0 => {
                self.enable(KIND, editable);
                self.enable(TARGET, editable && self.kind() <= 2);
                for id in MODS {
                    self.enable(id, editable && self.kind() == 1);
                }
                self.enable(APPLY, editable);
            }
            1 => {
                let supported = self.state.as_ref().is_some_and(|s| s.macro_supported)
                    && editable
                    && self.binding().is_some_and(|b| b.layer == 0);
                for id in [
                    NAME, STEPS, EVENT, EVENT_KEY, DELAY, ADD, DELETE, UP, DOWN, APPLY,
                ] {
                    self.enable(id, supported);
                }
                if supported {
                    let delay = self.select(EVENT) == 2;
                    self.enable(EVENT_KEY, !delay);
                    self.enable(DELAY, delay);
                    let max = self.state.as_ref().unwrap().macro_max_events;
                    self.enable(ADD, self.events.len() < max);
                    let selected = self.select(STEPS);
                    self.enable(DELETE, selected < self.events.len());
                    self.enable(UP, selected > 0 && selected < self.events.len());
                    self.enable(
                        DOWN,
                        selected < self.events.len() && selected + 1 < self.events.len(),
                    );
                    self.enable(APPLY, !self.events.is_empty());
                }
            }
            _ => {
                let supported = ready && self.state.as_ref().is_some_and(|s| !s.colors.is_empty());
                self.enable(PICK, supported);
                self.enable(APPLY, supported && self.picked);
                self.enable(ALL, supported && self.picked);
            }
        }
    }
    unsafe fn sync_binding(&self) {
        let action = self.binding().map(|b| b.action.clone());
        let kind = match action {
            Some(BindingAction::Mouse(_)) => 0,
            Some(BindingAction::Keyboard { .. }) => 1,
            Some(BindingAction::Consumer(_)) => 2,
            Some(BindingAction::Disabled) => 3,
            Some(BindingAction::DpiCycle) => 4,
            _ => usize::MAX,
        };
        self.set_select(
            KIND,
            self.kinds()
                .iter()
                .position(|v| *v == kind)
                .unwrap_or(usize::MAX),
        );
        self.targets();
        if let Some(action) = action {
            match action {
                BindingAction::Mouse(n) => self.set_select(TARGET, n.saturating_sub(1) as usize),
                BindingAction::Keyboard { usage, modifiers } => {
                    self.set_select(
                        TARGET,
                        self.keys
                            .iter()
                            .position(|(u, _)| *u == usage)
                            .unwrap_or(usize::MAX),
                    );
                    for (i, mask) in [0x11, 0x22, 0x44, 0x88].iter().enumerate() {
                        SendMessageW(
                            self.child(MODS[i]),
                            BM_SETCHECK,
                            if modifiers & mask != 0 {
                                BST_CHECKED
                            } else {
                                BST_UNCHECKED
                            } as usize,
                            0,
                        );
                    }
                }
                BindingAction::Consumer(n) => self.set_select(
                    TARGET,
                    MEDIA
                        .iter()
                        .position(|(u, _)| *u == n)
                        .unwrap_or(usize::MAX),
                ),
                _ => {}
            }
        }
        self.availability();
    }
    unsafe fn targets(&self) {
        SendMessageW(self.child(TARGET), CB_RESETCONTENT, 0, 0);
        for id in MODS {
            SendMessageW(self.child(id), BM_SETCHECK, BST_UNCHECKED as usize, 0);
        }
        let options = match self.kind() {
            0 => (1..=5).map(|n| mouse_name(n).to_string()).collect(),
            1 => self.keys.iter().map(|(_, s)| s.clone()).collect(),
            2 => MEDIA.iter().map(|(_, s)| (*s).to_string()).collect(),
            _ => Vec::new(),
        };
        for option in options {
            SendMessageW(
                self.child(TARGET),
                CB_ADDSTRING,
                0,
                wide(&option).as_ptr() as isize,
            );
        }
        // Selecting a category must not pretend the first option was read from the device.
        self.set_select(TARGET, usize::MAX);
        self.availability();
    }
    unsafe fn remember_macro(&mut self) {
        if self.child(NAME).is_null() {
            return;
        }
        self.macro_name = self.text(NAME);
        if let Some(control) = self.macro_control {
            self.macro_drafts.insert(
                control,
                MacroDefinition {
                    name: self.macro_name.clone(),
                    mode: MacroMode::Once,
                    events: self.events.clone(),
                },
            );
        }
    }
    unsafe fn sync_macro(&mut self) {
        let control = self.binding().filter(|b| b.layer == 0).map(|b| b.control);
        if control != self.macro_control {
            self.macro_control = control;
            let definition = control.and_then(|control| {
                self.macro_drafts.get(&control).cloned().or_else(|| {
                    self.state
                        .as_ref()
                        .and_then(|s| s.macros.get(&control).cloned())
                })
            });
            if let Some(definition) = definition {
                self.macro_name = definition.name;
                self.events = definition.events;
            } else {
                self.macro_name = "我的宏".into();
                self.events.clear();
            }
        }
        self.set_text(NAME, &self.macro_name);
        self.sync_steps(None);
    }
    unsafe fn sync_steps(&self, selected: Option<usize>) {
        SendMessageW(self.child(STEPS), LB_RESETCONTENT, 0, 0);
        for (i, event) in self.events.iter().enumerate() {
            let label = match event {
                MacroEvent::Key { usage, pressed } => format!(
                    "{}  {}",
                    if *pressed { "按下" } else { "释放" },
                    self.key_name(*usage)
                ),
                MacroEvent::Delay(ms) => format!("等待 {ms} 毫秒"),
                _ => "未识别步骤（不可写入）".into(),
            };
            SendMessageW(
                self.child(STEPS),
                LB_ADDSTRING,
                0,
                wide(&format!("{:02}   {label}", i + 1)).as_ptr() as isize,
            );
        }
        if let Some(i) = selected {
            self.set_select(STEPS, i);
        }
    }
    unsafe fn sync_color(&mut self) {
        if let Some(color) = self
            .state
            .as_ref()
            .and_then(|s| s.colors.get(self.selected))
        {
            self.rgb = color.rgb;
            self.picked = true;
        } else {
            self.picked = false;
        }
        self.availability();
    }
    unsafe fn status(&mut self, hwnd: HWND, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.error = error;
        self.set_text(STATUS, &self.status);
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn begin(&mut self, hwnd: HWND, action: Option<ControlAction>) {
        if self.pending.is_some() || self.color_open {
            return;
        }
        self.saving = action.is_some();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status(
            hwnd,
            if self.saving {
                "正在保存到设备并读回确认，请稍候…"
            } else {
                "正在读取设备的真实键位和灯光…"
            },
            false,
        );
        self.availability();
        let model = self.model;
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = match action {
                Some(action) => controls::apply(model, &action),
                None => peripheral_desk::resident_macros::disable_device(REGISTRY[model].key)
                    .and_then(|_| controls::read(model)),
            };
            if tx.send(result).is_ok() {
                unsafe {
                    PostMessageW(window as HWND, FINISHED, 0, 0);
                }
            }
        });
    }
    unsafe fn finish(&mut self, hwnd: HWND) {
        let Some(rx) = self.pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(state)) => {
                let notice = state.notice.clone();
                self.state = Some(state);
                self.status = if self.saving {
                    format!("已保存，读回确认成功。{notice}")
                } else {
                    notice
                };
                self.error = false;
                self.layout(hwnd);
            }
            Ok(Err(e)) => {
                self.status(
                    hwnd,
                    format!("{}失败：{e}", if self.saving { "保存" } else { "读取" }),
                    true,
                );
                self.state = None;
                self.layout(hwnd);
            }
            Err(e) => {
                self.status(hwnd, format!("后台任务未返回结果：{e}"), true);
                self.state = None;
                self.layout(hwnd);
            }
        }
    }
    unsafe fn save_binding(&mut self, hwnd: HWND) {
        if !self.editable() {
            return;
        }
        let Some(binding) = self.binding() else {
            return;
        };
        let control = binding.control;
        let layer = binding.layer;
        let target = self.select(TARGET);
        let action = match self.kind() {
            0 if target < 5 => BindingAction::Mouse(target as u8 + 1),
            1 if target < self.keys.len() => {
                let mut modifiers = 0;
                for (i, mask) in [1, 2, 4, 8].iter().enumerate() {
                    if SendMessageW(self.child(MODS[i]), BM_GETCHECK, 0, 0) == BST_CHECKED as isize
                    {
                        modifiers |= mask;
                    }
                }
                BindingAction::Keyboard {
                    usage: self.keys[target].0,
                    modifiers,
                }
            }
            2 if target < MEDIA.len() => BindingAction::Consumer(MEDIA[target].0),
            3 => BindingAction::Disabled,
            4 => BindingAction::DpiCycle,
            _ => {
                self.status(hwnd, "请先选择具体的目标功能或键盘按键。", true);
                return;
            }
        };
        self.begin(
            hwnd,
            Some(ControlAction::Bind {
                control,
                layer,
                action,
            }),
        );
    }
    unsafe fn save_macro(&mut self, hwnd: HWND) {
        let Some(binding) = self.binding() else {
            return;
        };
        if !self.editable()
            || binding.layer != 0
            || !self.state.as_ref().is_some_and(|s| s.macro_supported)
        {
            return;
        }
        let control = binding.control;
        self.macro_name = self.text(NAME);
        if self.macro_name.trim().is_empty() {
            self.status(
                hwnd,
                "请为宏填写名称。A5 名称最多 20 个 UTF-8 字节（约 6 个汉字）。",
                true,
            );
            return;
        }
        self.begin(
            hwnd,
            Some(ControlAction::Macro {
                control,
                definition: MacroDefinition {
                    name: self.macro_name.clone(),
                    mode: MacroMode::Once,
                    events: self.events.clone(),
                },
            }),
        );
    }
    unsafe fn step(&mut self, hwnd: HWND, id: i32) {
        let selected = self.select(STEPS);
        match id {
            ADD => {
                let Some(state) = self.state.as_ref() else {
                    return;
                };
                if !state.macro_supported || self.events.len() >= state.macro_max_events {
                    return;
                }
                let event = match self.select(EVENT) {
                    0 | 1 => {
                        let Some((usage, _)) = self.keys.get(self.select(EVENT_KEY)) else {
                            self.status(hwnd, "请选择步骤的键盘按键。", true);
                            return;
                        };
                        MacroEvent::Key {
                            usage: *usage,
                            pressed: self.select(EVENT) == 0,
                        }
                    }
                    2 => {
                        let Ok(ms) = self.text(DELAY).parse::<u16>() else {
                            self.status(hwnd, "等待时间请输入 1–65535 毫秒。", true);
                            return;
                        };
                        if ms == 0 {
                            self.status(hwnd, "等待时间必须大于 0。", true);
                            return;
                        }
                        MacroEvent::Delay(ms)
                    }
                    _ => return,
                };
                let insert = if selected < self.events.len() {
                    selected + 1
                } else {
                    self.events.len()
                };
                self.events.insert(insert, event);
                self.sync_steps(Some(insert));
            }
            DELETE if selected < self.events.len() => {
                self.events.remove(selected);
                self.sync_steps(Some(selected.min(self.events.len().saturating_sub(1))));
            }
            UP if selected > 0 && selected < self.events.len() => {
                self.events.swap(selected, selected - 1);
                self.sync_steps(Some(selected - 1));
            }
            DOWN if selected < self.events.len() && selected + 1 < self.events.len() => {
                self.events.swap(selected, selected + 1);
                self.sync_steps(Some(selected + 1));
            }
            _ => {}
        }
        self.availability();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn choose(&mut self, hwnd: HWND) {
        if self.pending.is_some() || self.color_open || !self.picked {
            return;
        }
        let mut custom = [0u32; 16];
        let mut dialog: CHOOSECOLORW = zeroed();
        dialog.lStructSize = size_of::<CHOOSECOLORW>() as u32;
        dialog.hwndOwner = hwnd;
        dialog.rgbResult =
            self.rgb[0] as u32 | ((self.rgb[1] as u32) << 8) | ((self.rgb[2] as u32) << 16);
        dialog.lpCustColors = custom.as_mut_ptr();
        dialog.Flags = CC_FULLOPEN | CC_RGBINIT;
        self.color_open = true;
        let accepted = ChooseColorW(&mut dialog);
        self.color_open = false;
        if accepted != 0 {
            self.rgb = [
                dialog.rgbResult as u8,
                (dialog.rgbResult >> 8) as u8,
                (dialog.rgbResult >> 16) as u8,
            ];
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }
    unsafe fn save_colors(&mut self, hwnd: HWND, all: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(color) = state.colors.get(self.selected) else {
            return;
        };
        let desired = if all {
            state
                .colors
                .iter()
                .map(|c| KeyColor {
                    control: c.control,
                    rgb: self.rgb,
                })
                .collect()
        } else {
            vec![KeyColor {
                control: color.control,
                rgb: self.rgb,
            }]
        };
        self.begin(hwnd, Some(ControlAction::Colors(desired)));
    }
    unsafe fn command(&mut self, hwnd: HWND, id: i32, event: u32) {
        if self.pending.is_some() || self.color_open {
            return;
        }
        match id {
            10..=12 => {
                let page = (id - 10) as usize;
                if self.page == 2 || page == 2 {
                    self.selected = 0;
                }
                self.page = page;
                self.layout(hwnd);
                SetFocus(self.child(KEYS));
            }
            RELOAD => self.begin(hwnd, None),
            KEYS if event == LBN_SELCHANGE => {
                self.remember_macro();
                self.selected = self.select(KEYS);
                if self.page == 0 {
                    self.sync_binding();
                } else if self.page == 2 {
                    self.sync_color();
                } else {
                    self.sync_macro();
                    self.availability();
                }
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            }
            KIND if event == CBN_SELCHANGE => {
                self.targets();
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            }
            EVENT if event == CBN_SELCHANGE => self.availability(),
            STEPS if event == LBN_SELCHANGE => self.availability(),
            APPLY => match self.page {
                0 => self.save_binding(hwnd),
                1 => self.save_macro(hwnd),
                _ => self.save_colors(hwnd, false),
            },
            ADD | DELETE | UP | DOWN => self.step(hwnd, id),
            PICK => self.choose(hwnd),
            ALL => self.save_colors(hwnd, true),
            IDCANCEL => {
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            _ => {}
        }
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn wrapped(
        &self,
        c: &Canvas,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        lines: usize,
        color: u32,
    ) {
        let per = (width / 14).max(1) as usize;
        let chars = text.chars().collect::<Vec<_>>();
        for (i, chunk) in chars.chunks(per).take(lines).enumerate() {
            c.text(
                &chunk.iter().collect::<String>(),
                R::new(x, y + i as i32 * 25, width, 23),
                14,
                400,
                color,
                DT_LEFT,
            );
        }
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = zeroed();
        let dest = BeginPaint(hwnd, &mut ps);
        let mut rect = zeroed();
        GetClientRect(hwnd, &mut rect);
        let dc = CreateCompatibleDC(dest);
        let bitmap = CreateCompatibleBitmap(dest, rect.right.max(1), rect.bottom.max(1));
        let previous = SelectObject(dc, bitmap);
        {
            let c = Canvas::new(dc, self.dpi);
            c.rect(R::new(0, 0, WIDTH, HEIGHT), BG);
            c.gradient(R::new(0, 0, WIDTH, 76), 0xff201017, BG);
            c.text(
                "设备自定义",
                R::new(28, 18, 650, 30),
                23,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                REGISTRY[self.model].name,
                R::new(28, 52, 780, 21),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
            c.round(R::new(24, 144, 326, 512), 14, PANEL, Some(BORDER));
            c.round(R::new(366, 144, 650, 512), 14, PANEL, Some(BORDER));
            c.text(
                if self.page == 2 {
                    "选择灯光按键"
                } else {
                    "选择物理按键 / 分层"
                },
                R::new(40, 160, 294, 24),
                15,
                600,
                TEXT,
                DT_LEFT,
            );
            match self.page {
                0 => {
                    let title = self
                        .binding()
                        .map(|b| format!("{} · 第 {} 层", b.label, b.layer))
                        .unwrap_or_else(|| "尚无可用键位".into());
                    c.text(&title, R::new(390, 162, 598, 30), 20, 600, TEXT, DT_LEFT);
                    let value = self
                        .binding()
                        .map(|b| format!("当前：{}", self.describe(&b.action)))
                        .unwrap_or_else(|| "读取成功后显示设备实际配置".into());
                    c.text(&value, R::new(390, 202, 598, 24), 14, 400, MUTED, DT_LEFT);
                    c.text(
                        "操作类型",
                        R::new(390, 234, 320, 22),
                        13,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                    c.text(
                        "目标功能 / 按键",
                        R::new(390, 316, 414, 22),
                        13,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                    self.wrapped(&c,if self.editable(){"选择操作后点击应用。组合键在所选按键上叠加 Ctrl、Shift、Alt、Win；设备不支持的动作会给出原因。"}else{"此键当前只读。主点击、Fn、未知功能或已有宏会按设备规则保留，避免覆盖无法完整恢复的配置。"},390,450,592,4,DIM);
                }
                1 => {
                    c.text(
                        "创建单次宏",
                        R::new(390, 158, 598, 30),
                        20,
                        600,
                        TEXT,
                        DT_LEFT,
                    );
                    c.text("宏名称", R::new(390, 184, 292, 20), 13, 400, MUTED, DT_LEFT);
                    c.text(
                        "播放方式：单次",
                        R::new(718, 210, 270, 25),
                        14,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                    let max = self.state.as_ref().map_or(0, |s| s.macro_max_events);
                    c.text(
                        &format!("{} / {max} 步 · 每次按下必须有对应释放", self.events.len()),
                        R::new(390, 480, 598, 24),
                        13,
                        400,
                        MUTED,
                        DT_LEFT,
                    );
                    for (x, label) in [(390, "事件"), (532, "按键"), (720, "等待 / 毫秒")] {
                        c.text(label, R::new(x, 500, 170, 22), 13, 400, MUTED, DT_LEFT);
                    }
                    if !self.state.as_ref().is_some_and(|s| s.macro_supported) {
                        self.wrapped(
                            &c,
                            "该设备尚未开放宏写入，请查看下方原因；这里不会把软件宏冒充板载宏。",
                            390,
                            566,
                            426,
                            3,
                            ORANGE,
                        );
                    } else if !self.editable() {
                        self.wrapped(
                            &c,
                            "所选键位只读，请选择其他按键。已有设备宏暂不覆盖。",
                            390,
                            572,
                            426,
                            2,
                            ORANGE,
                        );
                    } else {
                        c.text(
                            "本工具宏可再次编辑；未知旧宏保持只读。",
                            R::new(390, 596, 426, 25),
                            13,
                            400,
                            DIM,
                            DT_LEFT,
                        );
                    }
                }
                _ => {
                    c.text(
                        "逐键灯光",
                        R::new(390, 162, 598, 30),
                        20,
                        600,
                        TEXT,
                        DT_LEFT,
                    );
                    if self.picked {
                        let rgb = 0xff000000
                            | ((self.rgb[0] as u32) << 16)
                            | ((self.rgb[1] as u32) << 8)
                            | self.rgb[2] as u32;
                        c.round(R::new(390, 224, 104, 104), 14, rgb, Some(BORDER));
                        c.text(
                            &format!("#{:02X}{:02X}{:02X}", self.rgb[0], self.rgb[1], self.rgb[2]),
                            R::new(516, 244, 440, 35),
                            25,
                            400,
                            TEXT,
                            DT_LEFT,
                        );
                        c.text(
                            "预览颜色，点击应用后写入设备",
                            R::new(516, 284, 440, 24),
                            13,
                            400,
                            MUTED,
                            DT_LEFT,
                        );
                        self.wrapped(&c,"应用到选中键只修改当前键；应用到全部键使用同一颜色。其它键位与设置由适配器保留。",390,428,598,4,DIM);
                    } else {
                        self.wrapped(&c,"此设备未返回可编辑的逐键灯数据。可能没有此功能，或当前连接暂不支持读取；具体原因见下方。",390,224,598,5,MUTED);
                    }
                }
            }
            c.text(
                if self.error {
                    "操作未完成 · 下方可滚动查看详细原因"
                } else if self.pending.is_some() {
                    "设备通信中 · 请等待完成后关闭"
                } else {
                    "设备状态 / 操作结果"
                },
                R::new(28, 654, 984, 22),
                12,
                400,
                if self.error { ORANGE } else { DIM },
                DT_LEFT,
            );
        }
        BitBlt(dest, 0, 0, rect.right, rect.bottom, dc, 0, 0, SRCCOPY);
        SelectObject(dc, previous);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
    unsafe fn draw_button(&self, item: &DRAWITEMSTRUCT) {
        let Some(child) = self.children.iter().find(|c| c.hwnd == item.hwndItem) else {
            return;
        };
        // Clear every physical pixel, including antialiased rounded corners.
        let background = if child.id < 100 || child.id == RELOAD {
            BG
        } else {
            PANEL
        };
        let brush = CreateSolidBrush(
            ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
        );
        FillRect(item.hDC, &item.rcItem, brush);
        DeleteObject(brush);
        let c = Canvas::new(item.hDC, self.dpi);
        let width = (item.rcItem.right - item.rcItem.left) * 96 / self.dpi as i32;
        let height = (item.rcItem.bottom - item.rcItem.top) * 96 / self.dpi as i32;
        let r = R::new(0, 0, width, height);
        let selected = child.id == 10 + self.page as i32;
        let primary = matches!(child.id, APPLY | ALL);
        let disabled = item.itemState & ODS_DISABLED != 0;
        let pressed = item.itemState & ODS_SELECTED != 0;
        c.rect(
            r,
            if child.id < 100 || child.id == RELOAD {
                BG
            } else {
                PANEL
            },
        );
        c.round(
            R::new(0, 0, width - 1, height - 1),
            9,
            if disabled {
                FIELD
            } else if primary || selected {
                if pressed {
                    ORANGE
                } else {
                    RED
                }
            } else {
                FIELD
            },
            Some(if item.itemState & ODS_FOCUS != 0 {
                MUTED
            } else {
                BORDER
            }),
        );
        c.text(
            &child.text,
            r,
            14,
            if primary || selected { 600 } else { 400 },
            if disabled { DIM } else { TEXT },
            DT_CENTER,
        );
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let editor = &mut *ptr;
    match msg {
        WM_CREATE => {
            editor.layout(hwnd);
            editor.begin(hwnd, None);
            0
        }
        WM_PAINT => {
            editor.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MEASUREITEM => i32::from(paint::measure_native_choice(
            &mut *(l as *mut MEASUREITEMSTRUCT),
            editor.dpi,
        )) as isize,
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if paint::draw_native_choice(item, editor.dpi, editor.font) {
                return 1;
            }
            editor.draw_button(item);
            1
        }
        WM_COMMAND => {
            editor.command(hwnd, (w & 0xffff) as i32, ((w >> 16) & 0xffff) as u32);
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        FINISHED => {
            editor.finish(hwnd);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            let dc = w as HDC;
            SetTextColor(dc, colorref(TEXT));
            SetBkColor(dc, colorref(FIELD));
            editor.brush as isize
        }
        WM_CLOSE => {
            if editor.pending.is_some() || editor.color_open {
                editor.status(
                    hwnd,
                    "设备通信或颜色选择尚未结束，请等待完成后关闭。",
                    false,
                );
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        // This child owns a nested message loop, never the application's quit message.
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// Main App keeps its own modal flag set until this function returns.
pub unsafe fn open(owner: HWND, model: usize) {
    if model >= REGISTRY.len() {
        return;
    }
    let dpi = GetDpiForWindow(owner).max(96);
    let instance = GetModuleHandleW(null());
    let name = wide("PeripheralDeskControls");
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&class);
    let font = CreateFontW(
        -(14 * dpi as i32 / 96),
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
    let mut editor = Box::new(Editor {
        model,
        dpi,
        font,
        brush: CreateSolidBrush(colorref(FIELD)),
        page: 0,
        children: Vec::new(),
        state: None,
        selected: 0,
        pending: None,
        saving: false,
        status: "正在准备设备编辑器…".into(),
        error: false,
        keys: key_names(),
        events: Vec::new(),
        macro_name: "我的宏".into(),
        macro_control: None,
        macro_drafts: BTreeMap::new(),
        rgb: [255, 56, 89],
        picked: false,
        color_open: false,
    });
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: editor.px(WIDTH),
        bottom: editor.px(HEIGHT),
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    AdjustWindowRectExForDpi(&mut frame, style, 0, 0, dpi);
    let mut parent = zeroed();
    GetWindowRect(owner, &mut parent);
    let x = (parent.left + (parent.right - parent.left - (frame.right - frame.left)) / 2).max(0);
    let y = (parent.top + (parent.bottom - parent.top - (frame.bottom - frame.top)) / 2).max(0);
    EnableWindow(owner, 0);
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        name.as_ptr(),
        wide(&format!("{} · 设备自定义", REGISTRY[model].name)).as_ptr(),
        style,
        x,
        y,
        frame.right - frame.left,
        frame.bottom - frame.top,
        owner,
        null_mut(),
        instance,
        editor.as_mut() as *mut Editor as _,
    );
    if !hwnd.is_null() {
        let dark = 1i32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as _,
            size_of::<i32>() as u32,
        );
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let mut message = zeroed();
        while IsWindow(hwnd) != 0 {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                if result == 0 {
                    PostQuitMessage(message.wParam as i32);
                }
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
    }
    EnableWindow(owner, 1);
    SetForegroundWindow(owner);
}
