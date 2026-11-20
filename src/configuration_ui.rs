//! Device configuration/diagnostic window. Import and hardware apply are separate.
use crate::paint::palette;
use crate::{
    configuration::{self, OperationReport},
    devices::{DeviceSpec, REGISTRY},
    hid::wide,
    paint::{self, Canvas, R},
};
use std::{
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
    sync::mpsc::{self, Receiver},
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{Dialogs::*, *},
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{EnableWindow, SetActiveWindow},
        WindowsAndMessaging::*,
    },
};
const DEVICE: usize = 201;
const EXPORT: usize = 202;
const IMPORT: usize = 203;
const APPLY: usize = 204;
const DIAGNOSTIC: usize = 205;
const MHUB_GUIDE: &str = "在「设备调节」选择要设置的迈从设备。本地档案可以在此导入和导出。";

fn available_actions(device: Option<&DeviceSpec>) -> &'static [usize] {
    match device {
        None => &[DIAGNOSTIC],
        Some(spec) if spec.mchose() => &[EXPORT, IMPORT, DIAGNOSTIC],
        Some(_) => &[EXPORT, IMPORT, APPLY, DIAGNOSTIC],
    }
}

struct Window {
    combo: HWND,
    log: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    brush: HBRUSH,
    dpi: u32,
    pending: Option<Receiver<Result<OperationReport, String>>>,
}
impl Window {
    unsafe fn device(&self) -> Option<&'static DeviceSpec> {
        let selection = usize::try_from(SendMessageW(self.combo, CB_GETCURSEL, 0, 0)).ok()?;
        REGISTRY.get(selection.checked_sub(1)?)
    }
    unsafe fn selection(&self, hwnd: HWND) {
        let device = self.device();
        let actions = available_actions(device);
        let width = ((864 - 24 * (actions.len() as i32 - 1)) / actions.len() as i32).min(272);
        let scale = |n: i32| n * self.dpi as i32 / 96;
        for &button in &self.buttons {
            let id = GetDlgCtrlID(button) as usize;
            if let Some(index) = actions.iter().position(|&action| action == id) {
                SetWindowPos(
                    button,
                    null_mut(),
                    scale(28 + (width + 24) * index as i32),
                    scale(386),
                    scale(width),
                    scale(40),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                ShowWindow(button, SW_HIDE);
            }
        }
        self.status(match device {
            Some(spec) if spec.mchose() => MHUB_GUIDE,
            Some(_) => "导入会恢复本地记录。要将支持的设置写回硬件，请选择「应用到设备」。",
            None => "可导出所有设备的匿名诊断。选择具体设备后可管理其本地配置。",
        });
        InvalidateRect(hwnd, null(), 0);
    }
    unsafe fn status(&self, text: &str) {
        SetWindowTextW(self.log, wide(text).as_ptr());
    }
    unsafe fn busy(&self, enabled: bool) {
        EnableWindow(self.combo, i32::from(enabled));
        for button in &self.buttons {
            EnableWindow(*button, i32::from(enabled));
        }
        RedrawWindow(
            GetParent(self.combo),
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN,
        );
    }
    unsafe fn run(&mut self, hwnd: HWND, id: usize) {
        let device = self.device();
        if self.pending.is_some() || !available_actions(device).contains(&id) {
            return;
        }
        let key = device.map(|spec| spec.key);
        let path = if id == APPLY {
            None
        } else {
            let filename = if id == DIAGNOSTIC {
                "PeripheralDesk-diagnostics.json".into()
            } else {
                format!("{}-configuration.json", key.unwrap())
            };
            match choose_file(hwnd, id == IMPORT, &filename) {
                Ok(Some(path)) => Some(path),
                Ok(None) => return,
                Err(error) => {
                    self.status(&error);
                    return;
                }
            }
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.busy(false);
        self.status(match id {
            EXPORT => "正在整理最近记录并导出配置…",
            IMPORT => "正在备份原记录并恢复本地配置，不写入硬件…",
            APPLY => "正在读取设备基线并逐项应用差异；完成前请保持设备连接…",
            _ => "正在生成匿名诊断，不包含路径、串号或宏内容…",
        });
        thread::spawn(move || {
            let result = match id {
                EXPORT => {
                    configuration::export_configuration(key.unwrap(), path.as_deref().unwrap())
                }
                IMPORT => {
                    configuration::import_configuration(key.unwrap(), path.as_deref().unwrap())
                }
                APPLY => configuration::apply_configuration(key.unwrap()),
                _ => configuration::export_diagnostics(key, path.as_deref().unwrap()),
            };
            let _ = tx.send(result);
        });
    }
    unsafe fn poll(&mut self) {
        let result = match self.pending.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("配置操作意外结束，请重新读取设备状态。".into())
            }
            _ => return,
        };
        self.pending = None;
        self.busy(true);
        match result {
            Ok(report) => {
                let mut text = report.message;
                for notice in report.skipped {
                    text.push_str("\r\n• ");
                    text.push_str(&notice);
                }
                self.status(&text);
            }
            Err(error) => self.status(&format!("操作未完成：{error}")),
        }
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let canvas = Canvas::new(dc, self.dpi);
        canvas.rect(R::new(0, 0, 920, 650), palette().bg);
        canvas.gradient(
            R::new(0, 0, 920, 3),
            palette().accent,
            palette().accent_light,
        );
        canvas.text(
            "配置与诊断",
            R::new(28, 24, 800, 38),
            26,
            600,
            palette().text,
            DT_LEFT,
        );
        canvas.text(
            "将设备配置留在手里，也让问题更容易定位",
            R::new(28, 68, 850, 24),
            14,
            400,
            palette().muted,
            DT_LEFT,
        );
        canvas.text(
            "选择设备",
            R::new(28, 111, 850, 22),
            13,
            600,
            palette().muted,
            DT_LEFT,
        );
        canvas.round(
            R::new(28, 198, 864, 164),
            14,
            palette().panel,
            Some(palette().border),
        );
        let device_action = match self.device() {
            None => (
                "导出诊断",
                "生成所有设备的匿名诊断，用于排查连接和读取问题。",
            ),
            Some(spec) if spec.mchose() => (
                "迈从设备调节",
                "在「设备调节」选择对应设备，即可打开它的板载设置。",
            ),
            Some(_) => ("应用到设备", "读取原值，再写入支持恢复的差异项。"),
        };
        for (y, title, description) in [
            (
                215,
                "导出配置",
                "保存最近缓存、已保存宏与预设。可在离线时使用。",
            ),
            (
                263,
                "导入本地",
                "先备份，再恢复本地记录；不会立即改变设备。",
            ),
            (311, device_action.0, device_action.1),
        ] {
            canvas.text(
                title,
                R::new(48, y, 130, 24),
                14,
                600,
                palette().text,
                DT_LEFT,
            );
            canvas.text(
                description,
                R::new(190, y, 674, 24),
                13,
                400,
                palette().muted,
                DT_LEFT,
            );
        }
        canvas.text(
            "操作结果",
            R::new(28, 446, 850, 22),
            13,
            600,
            palette().muted,
            DT_LEFT,
        );
        canvas.text(
            "诊断仅含匿名接口、插件版本与错误类别，不含设备串号、个人路径或原始宏。",
            R::new(28, 611, 864, 22),
            12,
            400,
            palette().dim,
            DT_LEFT,
        );
        drop(canvas);
        EndPaint(hwnd, &ps);
    }
}
pub(crate) unsafe fn choose_file(
    hwnd: HWND,
    import: bool,
    filename: &str,
) -> Result<Option<PathBuf>, String> {
    let mut file = [0u16; 32768];
    if !import {
        let text = wide(filename);
        file[..text.len()].copy_from_slice(&text);
    }
    let filter = wide("JSON 文件 (*.json)\0*.json\0\0");
    let extension = wide("json");
    let mut dialog: OPENFILENAMEW = zeroed();
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = hwnd;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.Flags = OFN_PATHMUSTEXIST
        | OFN_NOCHANGEDIR
        | OFN_EXPLORER
        | if import {
            OFN_FILEMUSTEXIST
        } else {
            OFN_OVERWRITEPROMPT
        };
    let accepted = if import {
        GetOpenFileNameW(&mut dialog)
    } else {
        GetSaveFileNameW(&mut dialog)
    };
    if accepted == 0 {
        let error = CommDlgExtendedError();
        return if error == 0 {
            Ok(None)
        } else {
            Err(format!("选择文件失败：{error}"))
        };
    }
    let end = file.iter().position(|&v| v == 0).unwrap_or(file.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(&file[..end]))))
}
unsafe fn text(hwnd: HWND) -> String {
    let mut buffer = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
    let count = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..count.max(0) as usize])
}
pub(crate) unsafe fn draw_action_button(item: &DRAWITEMSTRUCT, dpi: u32, primary: usize) -> isize {
    if item.CtlType != ODT_BUTTON {
        return 0;
    }
    // Clear every physical pixel, including antialiased rounded corners.
    let background = palette().bg;
    let brush = CreateSolidBrush(
        ((background >> 16) & 0xff) | (background & 0xff00) | ((background & 0xff) << 16),
    );
    FillRect(item.hDC, &item.rcItem, brush);
    DeleteObject(brush);
    let canvas = Canvas::new(item.hDC, dpi);
    let width = (item.rcItem.right - item.rcItem.left) * 96 / dpi as i32;
    let height = (item.rcItem.bottom - item.rcItem.top) * 96 / dpi as i32;
    let disabled = item.itemState & ODS_DISABLED != 0;
    canvas.round(
        R::new(0, 0, width, height),
        9,
        if item.CtlID as usize == primary && !disabled {
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
    canvas.text(
        &text(item.hwndItem),
        R::new(8, 0, width - 16, height),
        14,
        500,
        if disabled {
            palette().dim
        } else {
            palette().text
        },
        DT_CENTER,
    );
    1
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
            SetTextColor(w as HDC, crate::paint::colorref(palette().text));
            SetBkColor(w as HDC, crate::paint::colorref(palette().field));
            window.brush as isize
        }
        WM_MEASUREITEM => i32::from(paint::measure_native_choice(
            &mut *(l as *mut MEASUREITEMSTRUCT),
            window.dpi,
        )) as isize,
        WM_DRAWITEM => {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if paint::draw_native_choice(item, window.dpi, window.font) {
                return 1;
            }
            draw_action_button(item, window.dpi, APPLY)
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            let notice = (w >> 16) & 0xffff;
            if id == IDCANCEL as usize {
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
            } else if id == DEVICE && notice == CBN_SELCHANGE as usize {
                window.selection(hwnd);
            } else if notice == BN_CLICKED as usize {
                window.run(hwnd, id);
            }
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        WM_TIMER => {
            window.poll();
            0
        }
        WM_CLOSE => {
            if window.pending.is_some() {
                window.status("正在完成配置操作，请稍候。若写入正在进行，关闭可能影响恢复。");
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
pub unsafe fn open(owner: HWND, model: Option<usize>) {
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskConfiguration");
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
    let brush = CreateSolidBrush(crate::paint::colorref(palette().field));
    let mut window = Box::new(Window {
        combo: null_mut(),
        log: null_mut(),
        buttons: vec![],
        font,
        brush,
        dpi,
        pending: None,
    });
    let mut owner_rect: RECT = zeroed();
    GetWindowRect(owner, &mut owner_rect);
    let mut bounds = RECT {
        left: 0,
        top: 0,
        right: scale(920),
        bottom: scale(650),
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    AdjustWindowRectEx(&mut bounds, style, 0, WS_EX_DLGMODALFRAME);
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class.as_ptr(),
        wide("配置与诊断 · 外设盒").as_ptr(),
        style | WS_CLIPCHILDREN,
        owner_rect.left + ((owner_rect.right - owner_rect.left - width) / 2).max(0),
        owner_rect.top + ((owner_rect.bottom - owner_rect.top - height) / 2).max(0),
        width,
        height,
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
    let dark = i32::from(crate::paint::theme::is_dark());
    DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    let child = |class: &str, label: &str, id: usize, r: R, style: u32| {
        let child = CreateWindowExW(
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
            instance,
            null(),
        );
        SendMessageW(child, WM_SETFONT, font as usize, 1);
        if matches!(class, "EDIT" | "LISTBOX" | "COMBOBOX") {
            paint::theme_native_control(child, class, dpi);
        }
        child
    };
    window.combo = child(
        "COMBOBOX",
        "",
        DEVICE,
        R::new(28, 142, 864, 260),
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    SendMessageW(
        window.combo,
        CB_ADDSTRING,
        0,
        wide("所有设备（仅导出匿名诊断）").as_ptr() as isize,
    );
    for spec in REGISTRY.iter() {
        SendMessageW(
            window.combo,
            CB_ADDSTRING,
            0,
            wide(&format!("{}  ·  {}", spec.name, spec.key)).as_ptr() as isize,
        );
    }
    SendMessageW(
        window.combo,
        CB_SETCURSEL,
        model
            .filter(|index| *index < REGISTRY.len())
            .map_or(0, |index| index + 1),
        0,
    );
    for (id, label, x) in [
        (EXPORT, "导出配置", 28),
        (IMPORT, "导入本地", 250),
        (APPLY, "应用到设备", 472),
        (DIAGNOSTIC, "导出诊断", 694),
    ] {
        window.buttons.push(child(
            "BUTTON",
            label,
            id,
            R::new(x, 386, 198, 40),
            BS_OWNERDRAW as u32,
        ));
    }
    window.log = child(
        "EDIT",
        "",
        0,
        R::new(28, 478, 864, 114),
        ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_BORDER,
    );
    window.selection(hwnd);
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
                quit = Some(message.wParam as i32);
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
    DeleteObject(window.font);
    DeleteObject(window.brush);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::DeviceManifest;

    #[test]
    fn configuration_archive_actions_follow_the_device_editor() {
        for (brand, expected) in [
            ("MCHOSE", &[EXPORT, IMPORT, DIAGNOSTIC][..]),
            ("RAZER", &[EXPORT, IMPORT, APPLY, DIAGNOSTIC][..]),
        ] {
            let device = serde_json::from_value::<DeviceManifest>(serde_json::json!({
                "key":"test", "name":"Test", "brand":brand,
                "kind":"mouse", "matches":[{"vid":1,"pid":2}]
            }))
            .unwrap()
            .into_spec()
            .unwrap();
            assert_eq!(available_actions(Some(&device)), expected);
        }
        assert_eq!(available_actions(None), [DIAGNOSTIC]);
    }
}
