//! Window lifetime for the pointer editor inside the main device workspace.
use super::*;
use std::cell::Cell;

thread_local! {
    static CHILD: Cell<HWND> = const { Cell::new(null_mut()) };
}

pub(in crate::ui) unsafe fn open_embedded(
    owner: HWND,
    spec: &'static DeviceSpec,
    bounds: RECT,
) -> Result<(), String> {
    let current = CHILD.get();
    if !current.is_null() {
        let editor = GetWindowLongPtrW(current, GWLP_USERDATA) as *const Editor;
        if !editor.is_null() && (*editor).spec.key == spec.key {
            resize_embedded(bounds);
            return Ok(());
        }
        if !close_embedded() {
            return Err("正在保存指针设置，请等待读回完成。".into());
        }
    }
    let (hwnd, editor) = create(owner, spec, Some(bounds))?;
    // Embedded windows own their editor until WM_NCDESTROY. A modal editor
    // instead keeps its Box on the caller's stack to return the final sample.
    let editor = Box::into_raw(editor);
    CHILD.set(hwnd);
    ShowWindow(hwnd, SW_SHOWNA);
    (*editor).begin(hwnd, None);
    Ok(())
}

pub(in crate::ui) unsafe fn resize_embedded(bounds: RECT) {
    let hwnd = CHILD.get();
    if !hwnd.is_null() {
        SetWindowPos(
            hwnd,
            null_mut(),
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

pub(in crate::ui) unsafe fn embedded_writing() -> bool {
    let hwnd = CHILD.get();
    if hwnd.is_null() {
        return false;
    }
    let editor = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Editor;
    !editor.is_null() && (*editor).writing
}

pub(in crate::ui) unsafe fn close_embedded() -> bool {
    let hwnd = CHILD.get();
    if hwnd.is_null() {
        return true;
    }
    if embedded_writing() {
        return false;
    }
    DestroyWindow(hwnd) != 0
}

pub(super) unsafe fn destroyed(hwnd: HWND, data: *mut Editor) {
    if CHILD.get() == hwnd {
        CHILD.set(null_mut());
        drop(Box::from_raw(data));
    }
}
