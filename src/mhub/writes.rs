//! The G75 page and its native device lease survive navigation until every
//! accepted key edit has returned a device readback or an explicit failure.
use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

pub const WRITE_MESSAGE: u32 = WM_APP + 0xc9;
pub(super) const DRAINED: u32 = WM_APP + 0xca;
thread_local! {
    static LEAVING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<BTreeSet<u64>> = const { RefCell::new(BTreeSet::new()) };
    static NOTICES: RefCell<Vec<(String,bool)>> = const { RefCell::new(Vec::new()) };
}

pub fn closing() -> bool {
    LEAVING.get()
}

pub(super) fn pending() -> bool {
    PENDING.with_borrow(|p| !p.is_empty())
}

pub fn take_write_notices() -> Vec<(String, bool)> {
    NOTICES.take()
}

fn notice(hwnd: HWND, text: String, failed: bool) {
    NOTICES.with_borrow_mut(|v| v.push((text, failed)));
    unsafe {
        PostMessageW(GetParent(hwnd), WRITE_MESSAGE, 0, 0);
    }
}

pub(super) fn event(hwnd: HWND, value: &Value) {
    match value["event"].as_str() {
        Some("queued") => {
            if let Some(id) = value["id"].as_u64() {
                PENDING.with_borrow_mut(|pending| pending.insert(id));
            }
        }
        Some(kind @ ("confirmed" | "failed")) => {
            let Some(id) = value["id"].as_u64() else {
                return;
            };
            if PENDING.with_borrow_mut(|p| p.remove(&id)) {
                if kind == "confirmed" {
                    timing::mark("write_confirmed");
                    notice(hwnd, format!("G75 Pro 第 {id} 项改键已读回确认"), false);
                } else {
                    notice(
                        hwnd,
                        format!("G75 Pro 第 {id} 项改键未确认，请重新读取键盘"),
                        true,
                    );
                }
            }
        }
        Some("frame") => timing::mark("first_frame"),
        Some("editable") => timing::mark("editable"),
        Some("drained") if closing() => unsafe {
            PostMessageW(hwnd, DRAINED, 0, 0);
        },
        _ => {}
    }
}

pub(super) fn leave(hwnd: HWND) -> bool {
    if closing() {
        return true;
    }
    if !transport::managed_document() {
        return false;
    }
    LEAVING.set(true);
    // This callback is queued behind the last click in the document. Even if
    // its 'queued' web message has not reached Win32 yet, the edit is retained.
    let script = "if(window.PeripheralDeskWrites){window.PeripheralDeskWrites.finishLeave()}else{window.chrome.webview.postMessage({channel:'peripheral-desk-editor',event:'drained'})}";
    if run_local_script(script).is_ok() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
        return true;
    }
    LEAVING.set(false);
    false
}

pub(super) fn destroyed(hwnd: HWND) {
    for id in PENDING.take() {
        notice(
            hwnd,
            format!("G75 Pro 第 {id} 项改键尚未确认，请重新读取键盘"),
            true,
        );
    }
    LEAVING.set(false);
}
