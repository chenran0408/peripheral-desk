//! Native editor for reversible, desktop-resident keyboard macros.
use crate::{
    controls::{self, Binding, BindingAction, MacroEvent},
    devices::REGISTRY,
    hid::wide,
    paint::{self, Canvas, R},
};
use peripheral_desk::resident_macros::{self, Snapshot};
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
        Controls::*,
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{EnableWindow, SetActiveWindow},
        WindowsAndMessaging::*,
    },
};
const LIST: usize = 501;
const NAME: usize = 502;
const BINDING: usize = 503;
const EVENTS: usize = 504;
const NEW: usize = 505;
const SAVE: usize = 506;
const ENABLE: usize = 507;
const DISABLE: usize = 508;
const DELETE: usize = 509;
const RELOAD: usize = 510;
const LOG: usize = 511;
struct Outcome {
    state: Snapshot,
    focus: Option<u64>,
    bindings: Option<Vec<Binding>>,
    notice: String,
}
struct Window {
    key: String,
    model: usize,
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    list: HWND,
    name: HWND,
    binding: HWND,
    events: HWND,
    log: HWND,
    buttons: Vec<HWND>,
    state: Option<Snapshot>,
    rows: Vec<u64>,
    bindings: Vec<Binding>,
    selected: Option<u64>,
    pending: Option<Receiver<Result<Outcome, String>>>,
}
fn key_usage(name: &str) -> Option<u16> {
    let name = name.trim().to_ascii_uppercase();
    if let Some(value) = name.strip_prefix("HID:") {
        return value.parse().ok();
    }
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() {
            return Some(4 + u16::from(c - b'A'));
        }
        if (b'1'..=b'9').contains(&c) {
            return Some(30 + u16::from(c - b'1'));
        }
        if c == b'0' {
            return Some(39);
        }
    }
    if let Some(number) = name.strip_prefix('F').and_then(|s| s.parse::<u16>().ok()) {
        if (1..=12).contains(&number) {
            return Some(57 + number);
        }
    }
    Some(match name.as_str() {
        "ENTER" | "回车" => 40,
        "ESC" => 41,
        "BACKSPACE" | "退格" => 42,
        "TAB" => 43,
        "SPACE" | "空格" => 44,
        "CAPSLOCK" => 57,
        "INSERT" => 73,
        "HOME" => 74,
        "PAGEUP" => 75,
        "DELETE" => 76,
        "END" => 77,
        "PAGEDOWN" => 78,
        "RIGHT" | "右" => 79,
        "LEFT" | "左" => 80,
        "DOWN" | "下" => 81,
        "UP" | "上" => 82,
        "CTRL" | "LCTRL" => 224,
        "SHIFT" | "LSHIFT" => 225,
        "ALT" | "LALT" => 226,
        "WIN" | "LWIN" => 227,
        "RCTRL" => 228,
        "RSHIFT" => 229,
        "RALT" => 230,
        "RWIN" => 231,
        _ => return None,
    })
}
fn key_name(usage: u16) -> String {
    match usage {
        4..=29 => char::from(b'A' + (usage - 4) as u8).to_string(),
        30..=38 => (usage - 29).to_string(),
        39 => "0".into(),
        40 => "Enter".into(),
        41 => "Esc".into(),
        42 => "Backspace".into(),
        43 => "Tab".into(),
        44 => "Space".into(),
        58..=69 => format!("F{}", usage - 57),
        224 => "Ctrl".into(),
        225 => "Shift".into(),
        226 => "Alt".into(),
        227 => "Win".into(),
        228 => "RCtrl".into(),
        229 => "RShift".into(),
        230 => "RAlt".into(),
        231 => "RWin".into(),
        _ => format!("HID:{usage}"),
    }
}
fn parse_events(text: &str) -> Result<Vec<MacroEvent>, String> {
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(format!(
                "第 {} 行格式应为“按下 A / 释放 A / 延时 50”",
                index + 1
            ));
        }
        let command = parts[0].to_ascii_lowercase();
        let event = match command.as_str() {
            "按下" | "down" => MacroEvent::Key {
                usage: key_usage(parts[1])
                    .ok_or_else(|| format!("第 {} 行按键名称不支持", index + 1))?,
                pressed: true,
            },
            "释放" | "up" => MacroEvent::Key {
                usage: key_usage(parts[1])
                    .ok_or_else(|| format!("第 {} 行按键名称不支持", index + 1))?,
                pressed: false,
            },
            "延时" | "delay" => MacroEvent::Delay(
                parts[1]
                    .parse()
                    .map_err(|_| format!("第 {} 行延时应为毫秒整数", index + 1))?,
            ),
            _ => return Err(format!("第 {} 行操作不支持", index + 1)),
        };
        events.push(event);
    }
    resident_macros::validate_events(&events)?;
    Ok(events)
}
fn format_events(events: &[MacroEvent]) -> String {
    events
        .iter()
        .map(|e| match e {
            MacroEvent::Key { usage, pressed } => format!(
                "{} {}",
                if *pressed { "按下" } else { "释放" },
                key_name(*usage)
            ),
            MacroEvent::Delay(ms) => format!("延时 {ms}"),
            _ => "不支持的事件".into(),
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}
unsafe fn text(hwnd: HWND) -> String {
    let mut buffer = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
    let length = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}
impl Window {
    unsafe fn status(&self, value: &str) {
        SetWindowTextW(self.log, wide(value).as_ptr());
    }
    unsafe fn busy(&self, enabled: bool) {
        for child in [self.list, self.name, self.binding, self.events]
            .into_iter()
            .chain(self.buttons.iter().copied())
        {
            EnableWindow(child, i32::from(enabled));
        }
        RedrawWindow(
            GetParent(self.list),
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN,
        );
    }
    unsafe fn refresh_rows(&mut self, focus: Option<u64>) {
        SendMessageW(self.list, LB_RESETCONTENT, 0, 0);
        self.rows.clear();
        if let Some(snapshot) = &self.state {
            for definition in &snapshot.library.definitions {
                let status = if snapshot.active.contains(&definition.id) {
                    "运行中"
                } else if snapshot
                    .library
                    .recovery
                    .iter()
                    .any(|r| r.id == definition.id)
                {
                    "待恢复"
                } else {
                    "未启用"
                };
                let name = format!("{status} · {}", definition.name);
                SendMessageW(self.list, LB_ADDSTRING, 0, wide(&name).as_ptr() as isize);
                self.rows.push(definition.id);
            }
            for recovery in &snapshot.library.recovery {
                if !self.rows.contains(&recovery.id) {
                    SendMessageW(
                        self.list,
                        LB_ADDSTRING,
                        0,
                        wide(&format!("待恢复 · 原按键 #{}", recovery.id)).as_ptr() as isize,
                    );
                    self.rows.push(recovery.id);
                }
            }
        }
        if let Some(index) = focus.and_then(|id| self.rows.iter().position(|v| *v == id)) {
            SendMessageW(self.list, LB_SETCURSEL, index, 0);
            self.selection();
        } else {
            self.selected = None;
        }
    }
    unsafe fn refill_bindings(&self) {
        SendMessageW(self.binding, CB_RESETCONTENT, 0, 0);
        for binding in &self.bindings {
            let label = format!(
                "{} · {}",
                match binding.layer {
                    0 => "普通层",
                    1 => "Fn 层 1",
                    _ => "Fn 层 2",
                },
                binding.label
            );
            SendMessageW(
                self.binding,
                CB_ADDSTRING,
                0,
                wide(&label).as_ptr() as isize,
            );
        }
        if !self.bindings.is_empty() {
            SendMessageW(self.binding, CB_SETCURSEL, 0, 0);
        }
    }
    unsafe fn selection(&mut self) {
        let index = SendMessageW(self.list, LB_GETCURSEL, 0, 0);
        self.selected = self.rows.get(index as usize).copied();
        if let Some(definition) = self.state.as_ref().and_then(|s| {
            s.library
                .definitions
                .iter()
                .find(|d| Some(d.id) == self.selected)
        }) {
            SetWindowTextW(self.name, wide(&definition.name).as_ptr());
            SetWindowTextW(
                self.events,
                wide(&format_events(&definition.events)).as_ptr(),
            );
            if let Some(index) = self
                .bindings
                .iter()
                .position(|b| b.control == definition.control && b.layer == definition.layer)
            {
                SendMessageW(self.binding, CB_SETCURSEL, index, 0);
            }
        }
    }
    unsafe fn command(&mut self, hwnd: HWND, id: usize) {
        if id == IDCANCEL as usize {
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            return;
        }
        if self.pending.is_some() {
            return;
        }
        if id == NEW {
            self.selected = None;
            SendMessageW(self.list, LB_SETCURSEL, usize::MAX, 0);
            SetWindowTextW(self.name, wide("我的软件宏").as_ptr());
            SetWindowTextW(self.events, wide("按下 A\r\n延时 50\r\n释放 A").as_ptr());
            self.status("编辑后保存。仅启用会改写设备触发键，保存不会播放。");
            return;
        }
        if ![SAVE, ENABLE, DISABLE, DELETE, RELOAD].contains(&id) {
            return;
        }
        let selected = self.selected;
        let key = self.key.clone();
        let model = self.model;
        if [ENABLE, DISABLE, DELETE].contains(&id) && selected.is_none() {
            self.status("先选择一个已保存的软件宏或待恢复记录。");
            return;
        }
        let mut saving = None;
        if id == SAVE {
            let index = SendMessageW(self.binding, CB_GETCURSEL, 0, 0);
            let Some(binding) = self.bindings.get(index as usize) else {
                self.status("尚未读到可用触发键，请唤醒设备并刷新。");
                return;
            };
            let events = match parse_events(&text(self.events)) {
                Ok(events) => events,
                Err(e) => {
                    self.status(&e);
                    return;
                }
            };
            saving = Some((binding.control, binding.layer, text(self.name), events));
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.busy(false);
        self.status(match id {
            ENABLE => "正在占用保留热键、保存原绑定并启用软件宏…",
            DISABLE => "正在停止播放并恢复原按键，可能需要等待设备回复…",
            SAVE => "正在保存本地软件宏定义…",
            DELETE => "正在删除本地宏定义…",
            _ => "正在读取宏与可用按键…",
        });
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                let mut focus = selected;
                let notice = match id {
                    SAVE => {
                        let (control, layer, name, events) = saving.unwrap();
                        focus = Some(resident_macros::save(
                            &key, selected, control, layer, &name, events,
                        )?);
                        "定义已保存；尚未启用，也未播放。".into()
                    }
                    ENABLE => {
                        resident_macros::enable(&key, selected.unwrap())?;
                        "软件宏已启用：按所选设备键播放一次，需保持主程序常驻。".into()
                    }
                    DISABLE => {
                        resident_macros::disable(&key, selected.unwrap())?;
                        "已停用并核对恢复原按键。".into()
                    }
                    DELETE => {
                        resident_macros::delete(&key, selected.unwrap())?;
                        focus = None;
                        "已删除本地定义。".into()
                    }
                    _ => "已刷新；有待恢复记录时请先停用/恢复。".into(),
                };
                let state = resident_macros::list(&key)?;
                let bindings = if id == RELOAD {
                    Some(
                        controls::read(model)?
                            .bindings
                            .into_iter()
                            .filter(|b| {
                                b.editable
                                    && !matches!(
                                        b.action,
                                        BindingAction::Mouse(1)
                                            | BindingAction::Unknown(_)
                                            | BindingAction::Macro(_)
                                    )
                                    && !(key == "mchose-a5-ultra" && b.control == 0)
                                    && !(key == "razer-deathadder-v3-pro" && b.control == 1)
                            })
                            .collect(),
                    )
                } else {
                    None
                };
                Ok(Outcome {
                    state,
                    focus,
                    bindings,
                    notice,
                })
            })();
            let _ = tx.send(result);
        });
    }
    unsafe fn poll(&mut self, hwnd: HWND) {
        let result = match self.pending.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(r)) => r,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("软件宏操作意外结束，请刷新并检查待恢复记录。".into())
            }
            _ => return,
        };
        self.pending = None;
        self.busy(true);
        match result {
            Ok(outcome) => {
                self.state = Some(outcome.state);
                if let Some(bindings) = outcome.bindings {
                    self.bindings = bindings;
                    self.refill_bindings();
                }
                self.refresh_rows(outcome.focus);
                self.status(&outcome.notice);
            }
            Err(e) => {
                self.status(&e);
                if let Ok(state) = resident_macros::list(&self.key) {
                    self.state = Some(state);
                    self.refresh_rows(self.selected);
                }
            }
        }
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let canvas = Canvas::new(dc, self.dpi);
        canvas.rect(R::new(0, 0, 1040, 730), paint::BG);
        canvas.gradient(R::new(0, 0, 1040, 3), paint::RED, paint::ORANGE);
        for (label, r, size, color) in [
            ("软件宏", R::new(28, 21, 700, 37), 26, paint::TEXT),
            (
                "本机常驻执行 · 单次键盘动作 · 停用或退出恢复原按键",
                R::new(28, 65, 980, 24),
                14,
                paint::MUTED,
            ),
            (
                "我的宏与恢复记录",
                R::new(28, 109, 260, 24),
                13,
                paint::MUTED,
            ),
            ("名称", R::new(334, 109, 150, 24), 13, paint::MUTED),
            ("触发按键", R::new(334, 188, 260, 24), 13, paint::MUTED),
            ("播放步骤", R::new(334, 266, 350, 24), 13, paint::MUTED),
            (
                "每行：按下 A / 释放 A / 延时 50；Ctrl、Shift、F1–F12 或 HID:编号",
                R::new(334, 298, 670, 24),
                11,
                paint::DIM,
            ),
            (
                "F13–F24 为保留触发；最多 128 步、总延时 30 秒。不能使用左键或物理 Fn。",
                R::new(28, 633, 980, 23),
                12,
                paint::DIM,
            ),
            (
                "程序需保持运行。若退出时设备断开，重开此页可继续恢复原绑定。",
                R::new(28, 661, 980, 23),
                12,
                paint::MUTED,
            ),
        ] {
            canvas.text(label, r, size, 400, color, DT_LEFT);
        }
        if let Some(state) = &self.state {
            let pending = state
                .library
                .recovery
                .iter()
                .filter(|r| !state.active.contains(&r.id))
                .count();
            canvas.text(
                &format!("运行中 {}  ·  待恢复 {}", state.active.len(), pending),
                R::new(720, 26, 290, 30),
                14,
                500,
                if pending > 0 {
                    paint::ORANGE
                } else {
                    paint::GREEN
                },
                DT_RIGHT,
            );
        }
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
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let window = &mut *pointer;
    match message {
        WM_PAINT => {
            window.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            SetTextColor(w as HDC, 0x00f5f2f2);
            SetBkColor(w as HDC, 0x00261e1e);
            window.brush as isize
        }
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if item.CtlType != ODT_BUTTON {
                return 0;
            }
            // Clear every physical pixel, including antialiased rounded corners.
            let background = paint::BG;
            let brush = CreateSolidBrush(
                ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
            );
            FillRect(item.hDC, &item.rcItem, brush);
            DeleteObject(brush);
            let canvas = Canvas::new(item.hDC, window.dpi);
            let width = (item.rcItem.right - item.rcItem.left) * 96 / window.dpi as i32;
            let height = (item.rcItem.bottom - item.rcItem.top) * 96 / window.dpi as i32;
            let disabled = item.itemState & ODS_DISABLED != 0;
            canvas.round(
                R::new(0, 0, width, height),
                8,
                if item.CtlID as usize == ENABLE && !disabled {
                    paint::RED
                } else {
                    paint::FIELD
                },
                Some(if item.itemState & ODS_FOCUS != 0 {
                    paint::MUTED
                } else {
                    paint::BORDER
                }),
            );
            canvas.text(
                &text(item.hwndItem),
                R::new(4, 0, width - 8, height),
                13,
                500,
                if disabled { paint::DIM } else { paint::TEXT },
                DT_CENTER,
            );
            1
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            let event = (w >> 16) & 0xffff;
            if id == LIST && event == LBN_SELCHANGE as usize {
                window.selection();
            } else if event == BN_CLICKED as usize {
                window.command(hwnd, id);
            }
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        WM_TIMER => {
            window.poll(hwnd);
            0
        }
        WM_CLOSE => {
            if window.pending.is_some() {
                window.status("正在完成写入或恢复，请等待后再关闭。");
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            0
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}
pub unsafe fn open(owner: HWND, model: usize) {
    let Some(spec) = REGISTRY.get(model) else {
        return;
    };
    let state = match resident_macros::list(spec.key) {
        Ok(s) => s,
        Err(e) => {
            MessageBoxW(
                owner,
                wide(&e).as_ptr(),
                wide("软件宏").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskResidentMacros");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    });
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
    let brush = CreateSolidBrush(0x00261e1e);
    let mut window = Box::new(Window {
        key: spec.key.into(),
        model,
        dpi,
        font,
        brush,
        list: null_mut(),
        name: null_mut(),
        binding: null_mut(),
        events: null_mut(),
        log: null_mut(),
        buttons: vec![],
        state: Some(state),
        rows: vec![],
        bindings: vec![],
        selected: None,
        pending: None,
    });
    let mut bounds = RECT {
        left: 0,
        top: 0,
        right: scale(1040),
        bottom: scale(706),
    };
    AdjustWindowRectEx(
        &mut bounds,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        0,
        WS_EX_DLGMODALFRAME,
    );
    let mut owner_rect: RECT = zeroed();
    GetWindowRect(owner, &mut owner_rect);
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class.as_ptr(),
        wide(&format!("软件宏 · {}", spec.name)).as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        owner_rect.left + scale(25),
        owner_rect.top + scale(20),
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        owner,
        null_mut(),
        instance,
        (&mut *window as *mut Window).cast(),
    );
    if hwnd.is_null() {
        DeleteObject(font);
        DeleteObject(brush);
        return;
    }
    let dark = 1i32;
    DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    let child = |class: &str, value: &str, id: usize, r: R, style: u32| {
        let handle = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(value).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            scale(r.x),
            scale(r.y),
            scale(r.w),
            scale(r.h),
            hwnd,
            id as HMENU,
            instance,
            null(),
        );
        SendMessageW(handle, WM_SETFONT, font as usize, 1);
        handle
    };
    window.list = child(
        "LISTBOX",
        "",
        LIST,
        R::new(28, 143, 278, 342),
        LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL | WS_BORDER,
    );
    window.name = child(
        "EDIT",
        "我的软件宏",
        NAME,
        R::new(334, 143, 674, 34),
        ES_AUTOHSCROLL as u32 | WS_BORDER,
    );
    SendMessageW(window.name, EM_SETLIMITTEXT, 32, 0);
    window.binding = child(
        "COMBOBOX",
        "",
        BINDING,
        R::new(334, 221, 674, 360),
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    window.events = child(
        "EDIT",
        "按下 A\r\n延时 50\r\n释放 A",
        EVENTS,
        R::new(334, 330, 674, 155),
        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32 | WS_VSCROLL | WS_BORDER,
    );
    SendMessageW(window.events, EM_SETLIMITTEXT, 16000, 0);
    for (id, label, r) in [
        (NEW, "新建", R::new(28, 498, 80, 36)),
        (SAVE, "保存定义", R::new(120, 498, 100, 36)),
        (ENABLE, "启用软件宏", R::new(334, 498, 140, 36)),
        (DISABLE, "停用 / 恢复原键", R::new(486, 498, 178, 36)),
        (DELETE, "删除", R::new(676, 498, 90, 36)),
        (RELOAD, "刷新", R::new(778, 498, 100, 36)),
    ] {
        window
            .buttons
            .push(child("BUTTON", label, id, r, BS_OWNERDRAW as u32));
    }
    window.log = child(
        "EDIT",
        "软件宏未自动启用。请选择设备键，编辑后保存，再手动启用。",
        LOG,
        R::new(28, 550, 980, 66),
        ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_BORDER,
    );
    window.refresh_rows(None);
    window.command(hwnd, RELOAD);
    SetTimer(hwnd, 1, 80, None);
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    let mut message: MSG = zeroed();
    let mut quit = None;
    while IsWindow(hwnd) != 0 {
        let result = GetMessageW(&mut message, null_mut(), 0, 0);
        if result <= 0 {
            if result == 0 {
                quit = Some(message.wParam as i32)
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
    EnableWindow(owner, 1);
    SetActiveWindow(owner);
    // All children have been destroyed; release the resources owned by this window.
    DeleteObject(window.font);
    DeleteObject(window.brush);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_codec_supports_shortcuts_and_reports_unbalanced_edits() {
        let input = "按下 Ctrl\n按下 C\n延时 50\n释放 C\n释放 Ctrl";
        let events = parse_events(input).unwrap();
        assert_eq!(format_events(&events).replace("\r\n", "\n"), input);
        assert!(parse_events("按下 F13\n释放 F13").is_err());
        assert!(parse_events("按下 A").is_err());
    }
}
