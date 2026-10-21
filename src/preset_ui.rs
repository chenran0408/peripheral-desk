//! Native EQ preset library: local naming plus verified device application.
use crate::{
    audio_fx,
    devices::REGISTRY,
    hid::wide,
    paint::{self, Canvas, R},
    profiles::{self, EqLibrary},
};
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
const LIST: usize = 101;
const NAME: usize = 102;
const APPLY: usize = 103;
const SAVE: usize = 104;
const RENAME: usize = 105;
const DELETE: usize = 106;
const CLOSE: usize = 107;
type WorkResult = Result<(EqLibrary, Option<u64>, String), String>;
struct Dialog {
    library: EqLibrary,
    list: HWND,
    name: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    brush: HBRUSH,
    dpi: u32,
    status: String,
    error: bool,
    pending: Option<Receiver<WorkResult>>,
}
impl Dialog {
    unsafe fn selected(&self) -> Option<u64> {
        let index = SendMessageW(self.list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            None
        } else {
            self.library.presets.get(index as usize).map(|p| p.id)
        }
    }
    unsafe fn refill(&mut self, id: Option<u64>) {
        SendMessageW(self.list, LB_RESETCONTENT, 0, 0);
        for preset in &self.library.presets {
            SendMessageW(
                self.list,
                LB_ADDSTRING,
                0,
                wide(&preset.name).as_ptr() as isize,
            );
        }
        let index = id
            .and_then(|id| self.library.presets.iter().position(|p| p.id == id))
            .unwrap_or(0);
        if !self.library.presets.is_empty() {
            SendMessageW(self.list, LB_SETCURSEL, index, 0);
            self.selection_changed();
        } else {
            SetWindowTextW(self.name, wide("我的 EQ").as_ptr());
        }
    }
    unsafe fn selection_changed(&self) {
        if let Some(id) = self.selected() {
            if let Some(preset) = self.library.presets.iter().find(|p| p.id == id) {
                SetWindowTextW(self.name, wide(&preset.name).as_ptr());
            }
        }
    }
    unsafe fn busy(&self, enabled: bool) {
        EnableWindow(self.list, i32::from(enabled));
        EnableWindow(self.name, i32::from(enabled));
        for button in &self.buttons {
            EnableWindow(*button, i32::from(enabled));
        }
        RedrawWindow(
            GetParent(self.list),
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN,
        );
    }
    unsafe fn command(&mut self, hwnd: HWND, id: usize) {
        if self.pending.is_some() {
            return;
        }
        if id == CLOSE || id == IDCANCEL as usize {
            DestroyWindow(hwnd);
            return;
        }
        let id = if id == IDOK as usize { APPLY } else { id };
        if ![APPLY, SAVE, RENAME, DELETE].contains(&id) {
            return;
        }
        let selected = self.selected();
        let name = window_text(self.name);
        if [APPLY, RENAME, DELETE].contains(&id) && selected.is_none() {
            self.status = "请先选择一套预设".into();
            self.error = true;
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            return;
        }
        if id == DELETE {
            let title = self
                .library
                .presets
                .iter()
                .find(|p| Some(p.id) == selected)
                .map(|p| p.name.as_str())
                .unwrap_or("");
            if MessageBoxW(
                hwnd,
                wide(&format!(
                    "删除“{title}”？已应用到耳机的 EQ 不会改变。\n此操作只移除本地预设。"
                ))
                .as_ptr(),
                wide("删除 EQ 预设").as_ptr(),
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
            ) != IDYES
            {
                return;
            }
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.busy(false);
        self.error = false;
        self.status = match id {
            APPLY => "正在切换 EQ，并逐项读回确认…",
            SAVE => "正在读取当前十段 EQ 并保存…",
            RENAME => "正在保存预设名称…",
            DELETE => "正在删除本地预设…",
            _ => return,
        }
        .into();
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        thread::spawn(move || {
            let result = (|| -> WorkResult {
                let (focus, notice) = match id {
                    APPLY => {
                        let id = selected.unwrap();
                        profiles::apply_eq_preset(id)?;
                        (Some(id), "已切换到所选 EQ，读回确认成功".into())
                    }
                    SAVE => {
                        let id = profiles::save_current_eq(&name)?;
                        (Some(id), "当前 EQ 已保存为新预设".into())
                    }
                    RENAME => {
                        let id = selected.unwrap();
                        profiles::rename_eq_preset(id, &name)?;
                        (Some(id), "预设已重命名".into())
                    }
                    DELETE => {
                        profiles::delete_eq_preset(selected.unwrap())?;
                        (None, "本地预设已删除，耳机 EQ 保持原值".into())
                    }
                    _ => return Err("未知操作".into()),
                };
                Ok((profiles::list_eq_presets()?, focus, notice))
            })();
            let _ = tx.send(result);
        });
    }
    unsafe fn poll(&mut self, hwnd: HWND) {
        let result = match self.pending.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("预设操作意外结束，请刷新耳机检查当前设置".into())
            }
            _ => return,
        };
        self.pending = None;
        self.busy(true);
        match result {
            Ok((library, focus, notice)) => {
                self.library = library;
                self.refill(focus);
                self.status = notice;
                self.error = false;
            }
            Err(error) => {
                MessageBoxW(
                    hwnd,
                    wide(&error).as_ptr(),
                    wide("EQ 预设操作失败").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
                self.status = error;
                self.error = true;
            }
        }
        RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let canvas = Canvas::new(dc, self.dpi);
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let width = client.right * 96 / self.dpi as i32;
        let height = client.bottom * 96 / self.dpi as i32;
        canvas.rect(R::new(0, 0, width, height), paint::BG);
        canvas.gradient(R::new(0, 0, width, 3), paint::RED, paint::ORANGE);
        canvas.text(
            "EQ 预设库",
            R::new(28, 22, 500, 36),
            26,
            600,
            paint::TEXT,
            DT_LEFT,
        );
        canvas.text(
            "保存多套均衡器，按场景切换",
            R::new(28, 63, 750, 24),
            14,
            400,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            "我的预设",
            R::new(28, 100, 235, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.round(
            R::new(282, 124, 650, 278),
            14,
            paint::PANEL,
            Some(paint::BORDER),
        );
        if let Some(preset) = self
            .selected()
            .and_then(|id| self.library.presets.iter().find(|p| p.id == id))
        {
            canvas.text(
                &preset.name,
                R::new(306, 139, 470, 30),
                20,
                600,
                paint::TEXT,
                DT_LEFT,
            );
            let enabled = preset.values.get(&crate::model::Tuning::EqEnabled) == Some(&1);
            canvas.text(
                if enabled {
                    "EQ 已启用"
                } else {
                    "EQ 关闭 · 保留频段"
                },
                R::new(770, 145, 140, 22),
                12,
                400,
                paint::MUTED,
                DT_RIGHT,
            );
            let zero_y = 260;
            canvas.line(310, zero_y, 904, zero_y, paint::BORDER, 1.);
            canvas.text(
                "+20",
                R::new(297, 185, 32, 20),
                10,
                400,
                paint::DIM,
                DT_LEFT,
            );
            canvas.text(
                "−20",
                R::new(297, 324, 32, 20),
                10,
                400,
                paint::DIM,
                DT_LEFT,
            );
            let mut previous = None;
            for (index, (key, label)) in audio_fx::EQ_BANDS.iter().enumerate() {
                let gain = i32::from(*preset.values.get(key).unwrap_or(&20)) - 20;
                let x = 344 + index as i32 * 59;
                let y = zero_y - gain * 3;
                if let Some((px, py)) = previous {
                    canvas.line(px, py, x, y, paint::RED, 2.5);
                }
                canvas.ellipse(R::new(x - 4, y - 4, 8, 8), paint::ORANGE);
                canvas.text(
                    &format!("{gain:+}"),
                    R::new(x - 22, 348, 44, 20),
                    11,
                    600,
                    paint::TEXT,
                    DT_CENTER,
                );
                canvas.text(
                    label,
                    R::new(x - 22, 370, 44, 20),
                    10,
                    400,
                    paint::DIM,
                    DT_CENTER,
                );
                previous = Some((x, y));
            }
        } else {
            canvas.text(
                "保存当前 EQ，建立第一套预设",
                R::new(314, 220, 580, 30),
                18,
                400,
                paint::MUTED,
                DT_CENTER,
            );
        }
        canvas.text(
            "预设名称",
            R::new(28, 423, 400, 22),
            13,
            600,
            paint::MUTED,
            DT_LEFT,
        );
        let record = self
            .library
            .last_applied
            .and_then(|id| self.library.presets.iter().find(|p| p.id == id))
            .map(|p| format!("最近保存 / 应用：{}", p.name))
            .unwrap_or_else(|| "尚未应用预设".into());
        canvas.text(
            &record,
            R::new(28, 498, 890, 24),
            13,
            400,
            paint::MUTED,
            DT_LEFT,
        );
        canvas.text(
            &self.status,
            R::new(28, 532, 890, 24),
            13,
            400,
            if self.error { paint::RED } else { paint::GREEN },
            DT_LEFT,
        );
        canvas.text(
            "仅十段 EQ 和开关；其他音效与系统音量保持不变。",
            R::new(28, 570, 890, 21),
            12,
            400,
            paint::DIM,
            DT_LEFT,
        );
        canvas.text(
            "内置曲线可按听感继续调整；最近记录不代表外部驱动修改后的实时状态。",
            R::new(28, 593, 890, 21),
            11,
            400,
            paint::DIM,
            DT_LEFT,
        );
        drop(canvas);
        EndPaint(hwnd, &ps);
    }
}
unsafe fn window_text(hwnd: HWND) -> String {
    let count = GetWindowTextLengthW(hwnd).max(0) as usize;
    let mut buffer = vec![0; count + 1];
    let count = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..count.max(0) as usize])
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
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let dialog = &mut *pointer;
    match message {
        WM_PAINT => {
            dialog.paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT => {
            SetTextColor(w as HDC, 0x00f5f2f2);
            SetBkColor(w as HDC, 0x00261e1e);
            dialog.brush as isize
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
            let canvas = Canvas::new(item.hDC, dialog.dpi);
            let width = (item.rcItem.right - item.rcItem.left) * 96 / dialog.dpi as i32;
            let height = (item.rcItem.bottom - item.rcItem.top) * 96 / dialog.dpi as i32;
            let active = item.CtlID as usize == APPLY;
            let disabled = item.itemState & ODS_DISABLED != 0;
            canvas.round(
                R::new(0, 0, width, height),
                9,
                if active && !disabled {
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
                &window_text(item.hwndItem),
                R::new(8, 0, width - 16, height),
                13,
                500,
                if disabled { paint::DIM } else { paint::TEXT },
                DT_CENTER,
            );
            1
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            let notice = (w >> 16) & 0xffff;
            if id == LIST && notice == LBN_SELCHANGE as usize {
                dialog.selection_changed();
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            } else if notice == BN_CLICKED as usize {
                dialog.command(hwnd, id);
            }
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            0
        }
        WM_TIMER => {
            dialog.poll(hwnd);
            0
        }
        WM_CLOSE => {
            if dialog.pending.is_some() {
                dialog.status = "正在完成预设操作，请稍候再关闭".into();
                RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_KEYDOWN if w == 27 => {
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
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
    if REGISTRY
        .get(model)
        .is_none_or(|spec| spec.key != "hecate-g2-pro")
    {
        return;
    }
    let library = match profiles::list_eq_presets() {
        Ok(library) => library,
        Err(error) => {
            MessageBoxW(
                owner,
                wide(&error).as_ptr(),
                wide("无法打开 EQ 预设库").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
            return;
        }
    };
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let instance = GetModuleHandleW(null());
    let class = wide("PeripheralDeskEqPresets");
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);
    let font = CreateFontW(
        -scale(15),
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
    let mut dialog = Box::new(Dialog {
        library,
        list: null_mut(),
        name: null_mut(),
        buttons: vec![],
        font,
        brush,
        dpi,
        status: "选择预设后点击“应用预设”；保存当前不会更改音效。".into(),
        error: false,
        pending: None,
    });
    let mut owner_rect: RECT = zeroed();
    GetWindowRect(owner, &mut owner_rect);
    let mut bounds = RECT {
        left: 0,
        top: 0,
        right: scale(960),
        bottom: scale(630),
    };
    AdjustWindowRectEx(
        &mut bounds,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        0,
        WS_EX_DLGMODALFRAME,
    );
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let x = owner_rect.left + ((owner_rect.right - owner_rect.left - width) / 2).max(0);
    let y = owner_rect.top + ((owner_rect.bottom - owner_rect.top - height) / 2).max(0);
    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class.as_ptr(),
        wide("EQ 预设库 · G2 Pro").as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        instance,
        (&mut *dialog as *mut Dialog).cast(),
    );
    if hwnd.is_null() {
        DeleteObject(font);
        DeleteObject(brush);
        return;
    }
    let dark = 1i32;
    DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
    let child = |class: &str, text: &str, id: usize, r: R, style: u32| {
        let handle = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
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
    dialog.list = child(
        "LISTBOX",
        "",
        LIST,
        R::new(28, 124, 232, 278),
        LBS_NOTIFY as u32 | WS_VSCROLL | LBS_NOINTEGRALHEIGHT as u32 | WS_BORDER,
    );
    dialog.name = child(
        "EDIT",
        "",
        NAME,
        R::new(28, 451, 390, 36),
        ES_AUTOHSCROLL as u32 | WS_BORDER,
    );
    SendMessageW(dialog.name, EM_SETLIMITTEXT, 32, 0);
    for (id, text, r) in [
        (SAVE, "保存当前", R::new(434, 451, 118, 36)),
        (RENAME, "重命名", R::new(564, 451, 104, 36)),
        (DELETE, "删除", R::new(680, 451, 88, 36)),
        (APPLY, "应用预设", R::new(780, 451, 152, 36)),
    ] {
        dialog
            .buttons
            .push(child("BUTTON", text, id, r, BS_OWNERDRAW as u32));
    }
    dialog.refill(dialog.library.last_applied);
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
    DeleteObject(dialog.font);
    DeleteObject(dialog.brush);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
}
