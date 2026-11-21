//! The store lives for the main window's lifetime, including while its page is hidden.
use super::*;
use std::cell::RefCell;

thread_local! {
    static PAGE: RefCell<Option<(HWND, Box<Manager>)>> = const { RefCell::new(None) };
}

fn window() -> HWND {
    PAGE.with_borrow(|page| page.as_ref().map_or(null_mut(), |(hwnd, _)| *hwnd))
}

fn font(dpi: u32) -> HFONT {
    unsafe {
        CreateFontW(
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
        )
    }
}

pub(super) unsafe fn update_appearance(hwnd: HWND, manager: &mut Manager) {
    let dpi = GetDpiForWindow(hwnd).max(96);
    let old_font = if manager.dpi != dpi {
        manager.dpi = dpi;
        Some(std::mem::replace(&mut manager.font, font(dpi)))
    } else {
        None
    };
    let brush = CreateSolidBrush(colorref(palette().field));
    DeleteObject(std::mem::replace(&mut manager.brush, brush));
    for child in &manager.children {
        if old_font.is_some() {
            SendMessageW(child.hwnd, WM_SETFONT, manager.font as usize, 0);
        }
        let control = match child.id {
            LIST => "LISTBOX",
            BRAND => "COMBOBOX",
            SEARCH | DESCRIPTION | STATUS => "EDIT",
            _ => continue,
        };
        crate::paint::theme_native_control(child.hwnd, control, dpi);
    }
    if let Some(old) = old_font {
        DeleteObject(old);
    }
    manager.reflow(hwnd);
    RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
}

pub(crate) unsafe fn theme_changed() {
    let hwnd = window();
    if !hwnd.is_null() {
        update_appearance(
            hwnd,
            &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Manager),
        );
    }
}

pub(crate) unsafe fn busy() -> bool {
    PAGE.with_borrow(|page| {
        page.as_ref()
            .is_some_and(|(_, manager)| manager.blocking || manager.dialog_open)
    })
}

pub(super) fn destroyed(hwnd: HWND) {
    if window() == hwnd {
        PAGE.with_borrow_mut(|page| *page = None);
    }
}

pub(crate) unsafe fn shutdown() {
    let hwnd = window();
    if !hwnd.is_null() {
        DestroyWindow(hwnd);
    }
}

pub(crate) unsafe fn set_visible(visible: bool) {
    let hwnd = window();
    if !hwnd.is_null() && (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0) != visible {
        if !visible && IsChild(hwnd, GetFocus()) != 0 {
            SetFocus(GetParent(hwnd));
        }
        ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

pub(crate) unsafe fn resize(bounds: RECT) {
    let hwnd = window();
    if !hwnd.is_null() {
        SetWindowPos(
            hwnd,
            null_mut(),
            bounds.left,
            bounds.top,
            (bounds.right - bounds.left).max(0),
            (bounds.bottom - bounds.top).max(0),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub(crate) unsafe fn pretranslate(message: &MSG) -> bool {
    if message.message != WM_KEYDOWN {
        return false;
    }
    let hwnd = window();
    if hwnd.is_null() || IsWindowVisible(hwnd) == 0 {
        return false;
    }
    let owner = GetParent(hwnd);
    if IsWindowEnabled(owner) == 0 || (message.hwnd != owner && IsChild(owner, message.hwnd) == 0) {
        return false;
    }
    if message.wParam == usize::from(b'F') && GetKeyState(VK_CONTROL as i32) < 0 {
        let search = GetDlgItem(hwnd, SEARCH);
        if IsWindowEnabled(search) != 0 {
            SetFocus(search);
            SendMessageW(search, EM_SETSEL, 0, -1);
            return true;
        }
    } else if message.wParam == VK_ESCAPE as usize {
        let brand = GetDlgItem(hwnd, BRAND);
        if SendMessageW(brand, CB_GETDROPPEDSTATE, 0, 0) != 0 {
            SendMessageW(brand, CB_SHOWDROPDOWN, 0, 0);
        } else {
            PostMessageW(GetParent(hwnd), BACK, 0, 0);
        }
        return true;
    }
    // The main message loop handles Tab and mnemonics, across the sidebar and this subtree.
    false
}

pub(crate) unsafe fn mount(
    owner: HWND,
    bounds: RECT,
    brand: Option<&str>,
    updates: bool,
) -> Result<(), String> {
    let hwnd = window();
    if !hwnd.is_null() {
        resize(bounds);
        let hidden = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE == 0;
        let manager = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Manager);
        if brand.is_some() || updates {
            manager.browser.brand = brand.map(str::to_owned);
            if !manager.browser.query.is_empty() {
                manager.browser.query.clear();
                manager.text(SEARCH, "");
            }
            manager.layout(hwnd);
        }
        if updates {
            if manager.pending.is_some() {
                manager.queued_update_check = true;
            } else {
                manager.select_updates_after_refresh = true;
                manager.begin(hwnd, Operation::Refresh);
            }
        } else if hidden && manager.pending.is_none() && !manager.changed {
            manager.begin(hwnd, Operation::Load);
        }
        set_visible(true);
        return Ok(());
    }

    let dpi = GetDpiForWindow(owner).max(96);
    let instance = GetModuleHandleW(null());
    let class_name = wide("PeripheralDeskPluginPage");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class_name.as_ptr(),
        ..zeroed()
    });
    let mut manager = Box::new(Manager {
        dpi,
        font: font(dpi),
        brush: CreateSolidBrush(colorref(palette().field)),
        children: Vec::new(),
        inventory: None,
        selected: 0,
        browser: Browser {
            brand: brand.map(str::to_owned),
            ..Browser::default()
        },
        checked: BTreeSet::new(),
        pending: None,
        status: "正在读取插件目录…".into(),
        success: String::new(),
        error: false,
        dialog_open: false,
        changing_channel: false,
        select_updates_after_refresh: updates,
        changed: false,
        blocking: false,
        queued_update_check: false,
    });
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class_name.as_ptr(),
        wide("插件管理").as_ptr(),
        WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
        bounds.left,
        bounds.top,
        (bounds.right - bounds.left).max(0),
        (bounds.bottom - bounds.top).max(0),
        owner,
        null_mut(),
        instance,
        manager.as_mut() as *mut Manager as _,
    );
    if hwnd.is_null() {
        return Err(format!(
            "无法打开插件管理：{}",
            std::io::Error::last_os_error()
        ));
    }
    PAGE.with_borrow_mut(|page| *page = Some((hwnd, manager)));
    set_visible(true);
    Ok(())
}
