//! Scrollable host for native editors. The window owns its editor until destruction.
use crate::{hid::wide, paint};
use std::{
    cell::Cell,
    marker::PhantomData,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::SetScrollInfo,
        HiDpi::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

pub(crate) trait EditorState {
    fn blocks_close(&self) -> bool;
}

pub(crate) struct Embedded<T> {
    content: Cell<HWND>,
    model: Cell<usize>,
    state: PhantomData<T>,
}

impl<T: EditorState> Embedded<T> {
    pub(crate) const fn new() -> Self {
        Self {
            content: Cell::new(null_mut()),
            model: Cell::new(0),
            state: PhantomData,
        }
    }

    pub(crate) unsafe fn open(
        &self,
        owner: HWND,
        model: usize,
        bounds: RECT,
        create: impl FnOnce(HWND) -> Result<(HWND, Box<T>), String>,
    ) -> Result<(), String> {
        if !self.content.get().is_null() && self.model.get() == model {
            self.resize(bounds);
            return Ok(());
        }
        if !self.close() {
            return Err("请等待当前操作完成后再切换设备。".into());
        }
        let instance = GetModuleHandleW(null());
        let class = wide("PeripheralDeskEditorViewport");
        RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        });
        let host = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            null(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_VSCROLL | WS_HSCROLL,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            owner,
            null_mut(),
            instance,
            null(),
        );
        if host.is_null() {
            return Err(format!(
                "无法创建编辑区域：{}",
                std::io::Error::last_os_error()
            ));
        }
        let (hwnd, state) = match create(host) {
            Ok(content) => content,
            Err(error) => {
                DestroyWindow(host);
                return Err(error);
            }
        };
        let _ = Box::into_raw(state);
        self.model.set(model);
        self.content.set(hwnd);
        layout(host);
        ShowWindow(hwnd, SW_SHOWNA);
        ShowWindow(host, SW_SHOWNA);
        Ok(())
    }

    pub(super) fn content(&self) -> HWND {
        self.content.get()
    }
    pub(super) unsafe fn hide_and_detach(&self) -> HWND {
        let content = self.content.replace(null_mut());
        ShowWindow(GetParent(content), SW_HIDE);
        content
    }
    pub(super) unsafe fn resume(&self, content: HWND, model: usize, bounds: RECT) {
        self.model.set(model);
        self.content.set(content);
        self.resize(bounds);
        ShowWindow(GetParent(content), SW_SHOWNA);
    }
    pub(crate) unsafe fn resize(&self, bounds: RECT) {
        if !self.content.get().is_null() {
            SetWindowPos(
                GetParent(self.content.get()),
                null_mut(),
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }

    pub(crate) unsafe fn busy(&self) -> bool {
        let hwnd = self.content.get();
        !hwnd.is_null() && (*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const T)).blocks_close()
    }

    pub(crate) unsafe fn close(&self) -> bool {
        self.content.get().is_null()
            || (!self.busy() && DestroyWindow(GetParent(self.content.get())) != 0)
    }

    pub(crate) unsafe fn destroyed(&self, hwnd: HWND, state: *mut T) {
        if self.content.get() == hwnd {
            self.content.set(null_mut());
            drop(Box::from_raw(state));
        }
    }
}

unsafe fn position(host: HWND) {
    let child = GetWindow(host, GW_CHILD);
    if !child.is_null() {
        SetWindowPos(
            child,
            null_mut(),
            -GetScrollPos(host, SB_HORZ),
            -GetScrollPos(host, SB_VERT),
            0,
            0,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
        );
    }
}

pub(crate) unsafe fn follow_focus(control: HWND, editor: HWND) {
    SetWindowSubclass(control, Some(control_proc), 1, GetParent(editor) as usize);
}

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    host: usize,
) -> LRESULT {
    if msg == WM_SETFOCUS {
        let host = host as HWND;
        let mut rect: RECT = zeroed();
        let mut visible = zeroed();
        GetWindowRect(hwnd, &mut rect);
        MapWindowPoints(null_mut(), host, (&mut rect as *mut RECT).cast(), 2);
        GetClientRect(host, &mut visible);
        for (bar, start, end, page) in [
            (SB_HORZ, rect.left, rect.right, visible.right),
            (SB_VERT, rect.top, rect.bottom, visible.bottom),
        ] {
            let delta = if start < 0 {
                start
            } else {
                (end - page).max(0).min(start)
            };
            if delta != 0 {
                scroll(host, bar, -1, delta);
            }
        }
    } else if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(control_proc), id);
    }
    DefSubclassProc(hwnd, msg, w, l)
}

pub(super) unsafe fn content_size(child: HWND, width: i32, height: i32) {
    let mut rect = zeroed();
    GetWindowRect(child, &mut rect);
    if rect.right - rect.left != width || rect.bottom - rect.top != height {
        SetWindowPos(
            child,
            null_mut(),
            0,
            0,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE,
        );
        layout(GetParent(child));
    }
}
unsafe fn layout(host: HWND) {
    let child = GetWindow(host, GW_CHILD);
    if child.is_null() {
        return;
    }
    let mut content = zeroed();
    let mut outer = zeroed();
    GetWindowRect(child, &mut content);
    GetWindowRect(host, &mut outer);
    let (w, h) = (content.right - content.left, content.bottom - content.top);
    let (width, height) = (outer.right - outer.left, outer.bottom - outer.top);
    let dpi = GetDpiForWindow(host);
    let bar_w = GetSystemMetricsForDpi(SM_CXVSCROLL, dpi);
    let bar_h = GetSystemMetricsForDpi(SM_CYHSCROLL, dpi);
    let horizontal = w > width || (h > height && w > width - bar_w);
    let vertical = h > height || (horizontal && h > height - bar_h);
    for (bar, extent, visible) in [
        (SB_HORZ, w, if vertical { width - bar_w } else { width }),
        (SB_VERT, h, if horizontal { height - bar_h } else { height }),
    ] {
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE,
            nMax: extent - 1,
            nPage: visible.max(1) as u32,
            ..zeroed()
        };
        SetScrollInfo(host, bar, &info, 1);
    }
    position(host);
}

unsafe fn scroll(host: HWND, bar: SCROLLBAR_CONSTANTS, action: i32, delta: i32) {
    let mut info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..zeroed()
    };
    GetScrollInfo(host, bar, &mut info);
    let line = (24 * GetDpiForWindow(host) / 96) as i32;
    info.nPos = match action {
        SB_LINEUP => info.nPos - line,
        SB_LINEDOWN => info.nPos + line,
        SB_PAGEUP => info.nPos - info.nPage as i32,
        SB_PAGEDOWN => info.nPos + info.nPage as i32,
        SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
        SB_TOP => info.nMin,
        SB_BOTTOM => info.nMax,
        _ if delta != 0 => info.nPos + delta,
        _ => return,
    };
    info.fMask = SIF_POS;
    SetScrollInfo(host, bar, &info, 1);
    position(host);
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_VSCROLL | WM_HSCROLL => {
            scroll(
                hwnd,
                if msg == WM_VSCROLL { SB_VERT } else { SB_HORZ },
                (w & 0xffff) as i32,
                0,
            );
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = (w >> 16) as i16 as i32 * GetDpiForWindow(hwnd) as i32 / 192;
            let horizontal = msg == WM_MOUSEHWHEEL || (w & 4) != 0;
            scroll(
                hwnd,
                if horizontal { SB_HORZ } else { SB_VERT },
                -1,
                if msg == WM_MOUSEHWHEEL { delta } else { -delta },
            );
            0
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let brush = CreateSolidBrush(super::colorref(paint::BG));
            FillRect(dc, &ps.rcPaint, brush);
            DeleteObject(brush);
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct State(Cell<bool>);
    impl EditorState for State {
        fn blocks_close(&self) -> bool {
            self.0.get()
        }
    }
    thread_local! { static PAGE: Embedded<State> = const { Embedded::new() }; }

    unsafe extern "system" fn content_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_NCCREATE {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (*(l as *const CREATESTRUCTW)).lpCreateParams as isize,
            );
        } else if msg == WM_NCDESTROY {
            PAGE.with(|page| {
                page.destroyed(hwnd, GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State)
            });
        }
        DefWindowProcW(hwnd, msg, w, l)
    }

    #[test]
    fn embedded_native_page_scrolls_resizes_and_keeps_a_busy_editor_alive() {
        unsafe {
            let instance = GetModuleHandleW(null());
            let class = wide("PeripheralDeskViewportTestContent");
            RegisterClassExW(&WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(content_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..zeroed()
            });
            let owner = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                900,
                700,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            PAGE.with(|page| {
                let mut bounds = RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 200,
                };
                page.open(owner, 4, bounds, |host| {
                    let mut state = Box::new(State(Cell::new(true)));
                    let hwnd = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        null(),
                        WS_CHILD,
                        0,
                        0,
                        728,
                        520,
                        host,
                        null_mut(),
                        instance,
                        (&mut *state as *mut State).cast(),
                    );
                    assert!(!hwnd.is_null());
                    Ok((hwnd, state))
                })
                .unwrap();
                let content = page.content.get();
                let host = GetParent(content);
                assert_eq!(GetParent(host), owner);
                assert!(page.busy());
                assert!(!page.close());
                assert!(page.open(owner, 5, bounds, |_| unreachable!()).is_err());
                SendMessageW(host, WM_HSCROLL, SB_BOTTOM as usize, 0);
                SendMessageW(host, WM_VSCROLL, SB_BOTTOM as usize, 0);
                assert!(GetScrollPos(host, SB_HORZ) >= 428 && GetScrollPos(host, SB_VERT) >= 320);
                bounds.right = 800;
                bounds.bottom = 600;
                page.open(owner, 4, bounds, |_| unreachable!()).unwrap();
                assert_eq!(page.content.get(), content);
                assert_eq!(GetScrollPos(host, SB_HORZ), 0);
                assert_eq!(GetScrollPos(host, SB_VERT), 0);
                (*(GetWindowLongPtrW(content, GWLP_USERDATA) as *const State))
                    .0
                    .set(false);
                assert!(page.close());
                assert!(page.content.get().is_null());
                assert_eq!(IsWindow(host), 0);
            });
            DestroyWindow(owner);
        }
    }
}
