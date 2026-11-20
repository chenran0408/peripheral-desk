use crate::{
    configuration::templates::{self, Template},
    configuration_ui,
    devices::REGISTRY,
    hid::{wide, DeviceTarget},
    paint::{self, Canvas, R},
    plugin_manager,
};
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    sync::mpsc,
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::*,
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
        WindowsAndMessaging::*,
    },
};

pub(crate) use templates::supports;
const LIST: i32 = 111;
const NAME: i32 = 112;
const STATUS: i32 = 113;
const APPLY: i32 = 114;
const SAVE: i32 = 115;
const IMPORT: i32 = 116;
const EXPORT: i32 = 117;
const RENAME: i32 = 118;
const DELETE: i32 = 119;
const LEGACY: i32 = 120;
type Outcome = Result<(Vec<Template>, String, bool), String>;

struct Window {
    key: &'static str,
    model_name: &'static str,
    target: Option<DeviceTarget>,
    entries: Vec<Template>,
    dpi: u32,
    font: HFONT,
    brush: HBRUSH,
    pending: Option<mpsc::Receiver<Outcome>>,
    applying: bool,
    list_changed: bool,
    needs_refresh: bool,
}

impl Window {
    unsafe fn status(&self, hwnd: HWND, message: &str) {
        SetDlgItemTextW(hwnd, STATUS, wide(message).as_ptr());
    }

    unsafe fn selected(&self, hwnd: HWND) -> Option<&Template> {
        let index = SendDlgItemMessageW(hwnd, LIST, LB_GETCURSEL, 0, 0);
        self.entries.get(usize::try_from(index).ok()?)
    }

    unsafe fn refresh(&self, hwnd: HWND) {
        let idle = self.pending.is_none();
        let selected = self.selected(hwnd).is_some();
        for (id, enabled) in [
            (LIST, true),
            (NAME, true),
            (IMPORT, true),
            (LEGACY, templates::supports_legacy_import(self.key)),
            (EXPORT, selected),
            (RENAME, selected),
            (DELETE, selected),
            (APPLY, selected && self.target.is_some()),
            (SAVE, self.target.is_some()),
        ] {
            let child = GetDlgItem(hwnd, id);
            if !child.is_null() {
                EnableWindow(child, i32::from(idle && enabled));
            }
        }
    }

    unsafe fn add(&mut self, hwnd: HWND, entry: Template) -> usize {
        if let Some(index) = self
            .entries
            .iter()
            .position(|old| old.name.eq_ignore_ascii_case(&entry.name))
        {
            return index;
        }
        SendDlgItemMessageW(
            hwnd,
            LIST,
            LB_ADDSTRING,
            0,
            wide(&entry.name).as_ptr() as isize,
        );
        self.entries.push(entry);
        self.entries.len() - 1
    }

    unsafe fn initialize(&mut self, hwnd: HWND) {
        const WIDTH: i32 = 640;
        const HEIGHT: i32 = 406;
        let scale = |n| n * self.dpi as i32 / 96;
        let owner = GetParent(hwnd);
        let mut area: RECT = zeroed();
        GetWindowRect(owner, &mut area);
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: scale(WIDTH),
            bottom: scale(HEIGHT),
        };
        AdjustWindowRectEx(
            &mut bounds,
            GetWindowLongW(hwnd, GWL_STYLE) as u32,
            0,
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
        );
        let w = bounds.right - bounds.left;
        let h = bounds.bottom - bounds.top;
        SetWindowPos(
            hwnd,
            null_mut(),
            area.left + (area.right - area.left - w) / 2,
            area.top + (area.bottom - area.top - h) / 2,
            w,
            h,
            SWP_NOZORDER,
        );
        SetWindowTextW(
            hwnd,
            wide(&format!("{} · 本地模板", self.model_name)).as_ptr(),
        );
        let dark = i32::from(paint::theme::is_dark());
        DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
        let child = |id, class: &str, label: &str, r: R, style: u32| {
            let h = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(label).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | paint::native_control_style(class, style),
                scale(r.x),
                scale(r.y),
                scale(r.w),
                scale(r.h),
                hwnd,
                id as HMENU,
                GetModuleHandleW(null()),
                null(),
            );
            SendMessageW(h, WM_SETFONT, self.font as usize, 1);
            if class != "BUTTON" {
                paint::theme_native_control(h, class, self.dpi);
            }
        };
        child(
            LIST,
            "LISTBOX",
            "我的模板",
            R::new(24, 88, 228, 204),
            LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL,
        );
        child(
            NAME,
            "EDIT",
            "",
            R::new(276, 88, 340, 30),
            ES_AUTOHSCROLL as u32,
        );
        SendDlgItemMessageW(hwnd, NAME, EM_SETLIMITTEXT, 96, 0);
        for (id, title, r) in [
            (SAVE, "保存当前设备", R::new(276, 132, 130, 36)),
            (RENAME, "重命名", R::new(414, 132, 96, 36)),
            (DELETE, "删除", R::new(518, 132, 98, 36)),
            (IMPORT, "导入本地文件", R::new(24, 307, 110, 36)),
            (EXPORT, "导出模板", R::new(142, 307, 110, 36)),
            (APPLY, "应用所选模板", R::new(454, 307, 162, 36)),
        ] {
            child(id, "BUTTON", title, r, BS_OWNERDRAW as u32);
        }
        if templates::supports_legacy_import(self.key) {
            child(
                LEGACY,
                "BUTTON",
                "导入旧库",
                R::new(276, 307, 124, 36),
                BS_OWNERDRAW as u32,
            );
        }
        child(
            STATUS,
            "EDIT",
            "",
            R::new(276, 198, 340, 94),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        match templates::list(self.key) {
            Ok(list) => {
                for entry in list {
                    self.add(hwnd, entry);
                }
                if self.entries.is_empty() {
                    self.status(
                        hwnd,
                        "还没有本地模板。可导入模板文件，或连接设备后，将当前设置保存为模板。",
                    );
                } else {
                    SendDlgItemMessageW(hwnd, LIST, LB_SETCURSEL, 0, 0);
                    self.status(hwnd, &self.entries[0].detail);
                }
            }
            Err(error) => self.status(hwnd, &error),
        }
        self.refresh(hwnd);
        SetFocus(GetDlgItem(
            hwnd,
            if self.entries.is_empty() { NAME } else { LIST },
        ));
    }

    unsafe fn run(&mut self, hwnd: HWND, id: i32) {
        if self.pending.is_some() {
            return;
        }
        let selected = self.selected(hwnd).cloned();
        if id == DELETE {
            let Some(entry) = &selected else { return };
            let prompt = format!("删除本地模板「{}」？", entry.name);
            if MessageBoxW(
                hwnd,
                wide(&prompt).as_ptr(),
                wide("删除模板").as_ptr(),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
            ) != IDYES
            {
                return;
            }
        }
        let key = self.key;
        let product = self.model_name;
        let path = if matches!(id, IMPORT | EXPORT) {
            let file = format!("{key}-template.json");
            match configuration_ui::choose_file(hwnd, id == IMPORT, &file) {
                Ok(Some(path)) => Some(path),
                Ok(None) => return,
                Err(error) => {
                    self.status(hwnd, &error);
                    return;
                }
            }
        } else {
            None
        };
        let mut name = [0; 193];
        let size = GetDlgItemTextW(hwnd, NAME, name.as_mut_ptr(), name.len() as i32);
        let name = String::from_utf16_lossy(&name[..size as usize]);
        let target = self.target.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.applying = id == APPLY;
        self.list_changed = matches!(id, RENAME | DELETE);
        self.refresh(hwnd);
        self.status(
            hwnd,
            match id {
                SAVE => "正在从所选设备读取设置…",
                RENAME => "正在重命名…",
                DELETE => "正在删除本地模板…",
                APPLY => "正在核对当前设备，并写入不同的设置…",
                IMPORT => "正在读取本地模板文件…",
                LEGACY => "正在读取此型号的旧 M HUB 本地模板…",
                _ => "正在导出模板文件…",
            },
        );
        SetTimer(hwnd, 1, 80, None);
        thread::spawn(move || {
            let result: Outcome = (|| match id {
                SAVE => {
                    let entry = templates::capture(
                        key,
                        target.as_ref().ok_or("请先连接并选择设备。")?,
                        &name,
                    )?;
                    let note = format!("已保存「{}」。\r\n{}", entry.name, entry.detail);
                    Ok((vec![entry], note, false))
                }
                IMPORT => {
                    let entry = templates::import(key, &path.unwrap(), &name)?;
                    let note = format!("已导入「{}」。\r\n{}", entry.name, entry.detail);
                    Ok((vec![entry], note, false))
                }
                LEGACY => {
                    let (entries, note) = templates::import_legacy(key, product)?;
                    Ok((entries, note, false))
                }
                RENAME => {
                    let updated =
                        templates::rename(key, selected.as_ref().ok_or("请先选择模板。")?, &name)?;
                    Ok((vec![updated], "模板已重命名。".into(), false))
                }
                DELETE => {
                    templates::delete(selected.as_ref().ok_or("请先选择模板。")?)?;
                    Ok((vec![], "本地模板已删除。".into(), false))
                }
                EXPORT => {
                    templates::export(selected.as_ref().ok_or("请先选择模板。")?, &path.unwrap())?;
                    Ok((vec![], "模板文件已导出。".into(), false))
                }
                APPLY => {
                    let report = templates::apply(
                        key,
                        target.as_ref().ok_or("请先连接并选择设备。")?,
                        selected.as_ref().ok_or("请先选择模板。")?,
                    )?;
                    let message = std::iter::once(report.message)
                        .chain(report.skipped)
                        .collect::<Vec<_>>()
                        .join("\r\n");
                    Ok((vec![], message, report.changed != 0))
                }
                _ => unreachable!(),
            })();
            let _ = tx.send(result);
        });
    }

    unsafe fn poll(&mut self, hwnd: HWND) {
        let Some(rx) = &self.pending else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("模板操作意外中断。".into()),
        };
        KillTimer(hwnd, 1);
        self.pending = None;
        match result {
            Ok((entries, note, wrote)) => {
                self.needs_refresh |= wrote;
                if self.list_changed {
                    let index = SendDlgItemMessageW(hwnd, LIST, LB_GETCURSEL, 0, 0);
                    if let Some(index) = usize::try_from(index)
                        .ok()
                        .filter(|i| *i < self.entries.len())
                    {
                        self.entries.remove(index);
                        SendDlgItemMessageW(hwnd, LIST, LB_DELETESTRING, index, 0);
                    }
                    if !self.entries.is_empty() {
                        SendDlgItemMessageW(hwnd, LIST, LB_SETCURSEL, 0, 0);
                    }
                }
                for entry in entries {
                    let index = self.add(hwnd, entry);
                    SendDlgItemMessageW(hwnd, LIST, LB_SETCURSEL, index, 0);
                }
                self.status(hwnd, &note);
            }
            Err(error) => {
                self.needs_refresh |= self.applying;
                self.status(hwnd, &error);
            }
        }
        self.refresh(hwnd);
    }
}

unsafe extern "system" fn dialog(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, l);
        (*(l as *mut Window)).initialize(hwnd);
        return 0;
    }
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
    if data.is_null() {
        return 0;
    }
    let window = &mut *data;
    match msg {
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            let event = (w >> 16) as u32;
            match id {
                IDCANCEL => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                LIST if event == LBN_SELCHANGE => {
                    if let Some(entry) = window.selected(hwnd) {
                        window.status(hwnd, &entry.detail);
                    }
                    window.refresh(hwnd);
                }
                IMPORT | EXPORT | APPLY | SAVE | RENAME | DELETE | LEGACY
                    if event == BN_CLICKED =>
                {
                    window.run(hwnd, id)
                }
                _ => {}
            }
            1
        }
        WM_TIMER => {
            window.poll(hwnd);
            1
        }
        WM_CLOSE => {
            if window.pending.is_none() {
                EndDialog(hwnd, 0);
            } else {
                window.status(hwnd, "正在完成模板操作，完成后可以关闭。");
            }
            1
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let canvas = Canvas::new(dc, window.dpi);
            canvas.rect(R::new(0, 0, 640, 406), paint::palette().bg);
            canvas.text(
                "本地模板",
                R::new(24, 14, 185, 34),
                24,
                600,
                paint::palette().text,
                DT_LEFT,
            );
            let target = window
                .target
                .as_ref()
                .map_or("设备离线", |t| t.label.as_str());
            canvas.text(
                target,
                R::new(206, 18, 410, 28),
                13,
                400,
                paint::palette().muted,
                DT_RIGHT,
            );
            for (label, x, y) in [
                ("我的模板", 24, 58),
                ("模板名称（导入时可留空）", 276, 58),
                ("模板详情", 276, 172),
            ] {
                canvas.text(
                    label,
                    R::new(x, y, 200, 24),
                    14,
                    500,
                    paint::palette().text,
                    DT_LEFT,
                );
            }
            let hint = templates::hint(window.key);
            canvas.text(
                hint,
                R::new(24, 360, 592, 26),
                13,
                400,
                paint::palette().muted,
                DT_LEFT,
            );
            drop(canvas);
            EndPaint(hwnd, &ps);
            1
        }
        WM_MEASUREITEM => i32::from(paint::measure_native_choice(
            &mut *(l as *mut MEASUREITEMSTRUCT),
            window.dpi,
        )) as isize,
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if paint::draw_native_choice(item, window.dpi, window.font) {
                1
            } else {
                configuration_ui::draw_action_button(item, window.dpi, APPLY as usize)
            }
        }
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            SetTextColor(w as HDC, paint::colorref(paint::palette().text));
            SetBkColor(w as HDC, paint::colorref(paint::palette().field));
            window.brush as isize
        }
        _ => 0,
    }
}

// A zero-control standard dialog. Windows supplies modal focus and keyboard
// navigation; controls are created at the owner's DPI during WM_INITDIALOG.
#[repr(C, align(4))]
struct Layout {
    header: DLGTEMPLATE,
    menu: u16,
    class: u16,
    title: u16,
}
const LAYOUT: Layout = Layout {
    header: DLGTEMPLATE {
        style: WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_CLIPCHILDREN | DS_MODALFRAME as u32,
        dwExtendedStyle: 0,
        cdit: 0,
        x: 0,
        y: 0,
        cx: 320,
        cy: 200,
    },
    menu: 0,
    class: 0,
    title: 0,
};

pub(crate) unsafe fn open(owner: HWND, model: usize) -> bool {
    let Some(spec) = REGISTRY.get(model).filter(|s| supports(s.key)) else {
        return false;
    };
    let dpi = GetDpiForWindow(owner).max(96);
    let mut window = Window {
        key: spec.key,
        model_name: spec.name,
        target: plugin_manager::selected_device_target(spec.key),
        entries: Vec::new(),
        dpi,
        font: CreateFontW(
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
        ),
        brush: CreateSolidBrush(paint::colorref(paint::palette().field)),
        pending: None,
        applying: false,
        list_changed: false,
        needs_refresh: false,
    };
    let focus = GetFocus();
    let result = DialogBoxIndirectParamW(
        GetModuleHandleW(null()),
        &LAYOUT.header,
        owner,
        Some(dialog),
        (&mut window as *mut Window) as isize,
    );
    if result == -1 {
        let message = format!("无法打开本地模板：{}", std::io::Error::last_os_error());
        MessageBoxW(
            owner,
            wide(&message).as_ptr(),
            wide("本地模板").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    if !focus.is_null() && IsWindow(focus) != 0 {
        SetFocus(focus);
    }
    DeleteObject(window.font);
    DeleteObject(window.brush);
    window.needs_refresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

    #[test]
    fn offline_dialog_exposes_local_import_and_requires_a_target_for_writes() {
        unsafe {
            let mut window = Window {
                key: "mchose-a5-ultra",
                model_name: "A5 Ultra",
                target: None,
                entries: vec![],
                dpi: 96,
                font: GetStockObject(DEFAULT_GUI_FONT),
                brush: CreateSolidBrush(paint::colorref(paint::palette().field)),
                pending: None,
                applying: false,
                list_changed: false,
                needs_refresh: false,
            };
            let hwnd = CreateDialogIndirectParamW(
                GetModuleHandleW(null()),
                &LAYOUT.header,
                null_mut(),
                Some(dialog),
                (&mut window as *mut Window) as isize,
            );
            assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
            assert_eq!(IsWindowVisible(hwnd), 0);
            for id in [LIST, NAME, STATUS, APPLY, SAVE, IMPORT, EXPORT, LEGACY] {
                assert!(!GetDlgItem(hwnd, id).is_null());
            }
            assert_ne!(IsWindowEnabled(GetDlgItem(hwnd, IMPORT)), 0);
            assert_ne!(IsWindowEnabled(GetDlgItem(hwnd, LEGACY)), 0);
            assert_eq!(IsWindowEnabled(GetDlgItem(hwnd, APPLY)), 0);
            assert_eq!(IsWindowEnabled(GetDlgItem(hwnd, SAVE)), 0);
            DestroyWindow(hwnd);
            DeleteObject(window.brush);
        }
    }
}
