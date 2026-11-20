//! The local MCHOSE editor is a child of the main window on its UI apartment.
mod bootstrap;
mod builtin;
mod fixes;
mod menu;
mod release_resources;
mod resources;
mod workspace;
use crate::hid::wide;
use std::{
    cell::{Cell, RefCell},
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::{null, null_mut},
    rc::{Rc, Weak},
    sync::mpsc,
    thread,
};
use webview2_com::{
    take_pwstr, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    MoveFocusRequestedEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{E_POINTER, HWND as ComWindow, RECT as ComRect},
        System::Com::*,
    },
};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, RECT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_SHIFT},
        WindowsAndMessaging::*,
    },
};

const SETUP_PAGE: &str = "https://setup.peripheraldesk.test/mchose";
const TITLE: &str = "外设盒 · 迈从驱动中心";
pub const CLOSED_MESSAGE: u32 = WM_APP + 0x4d;
const SHOW_MENU: u32 = WM_APP + 0x4e;

struct Host(HWND);

thread_local! {
    static APARTMENT: Cell<bool> = const { Cell::new(false) };
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
    static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
    static ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn host_window() -> HWND {
    HOST.with_borrow(|host| host.as_ref().map_or(null_mut(), |host| host.0))
}

pub fn active() -> bool {
    !host_window().is_null()
}

pub fn take_error() -> Option<String> {
    ERROR.take()
}

/// # Safety
/// Call on the main UI thread with the target window from the current message.
pub unsafe fn owns_input(target: HWND) -> bool {
    let hwnd = host_window();
    !hwnd.is_null() && (hwnd == target || IsChild(hwnd, target) != 0)
}

pub fn set_bounds(bounds: RECT) {
    let hwnd = host_window();
    if !hwnd.is_null() {
        unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                bounds.left,
                bounds.top,
                (bounds.right - bounds.left).max(0),
                (bounds.bottom - bounds.top).max(0),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }
}

pub fn parent_moved() {
    if let Some(controller) = CONTROLLER.with_borrow(Clone::clone) {
        unsafe {
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }
}

pub fn show_menu(x: i32, y: i32) {
    let hwnd = host_window();
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, SHOW_MENU, x as usize, y as isize);
        }
    }
}

pub fn close() {
    let hwnd = host_window();
    if !hwnd.is_null() {
        unsafe {
            DestroyWindow(hwnd);
        }
    }
}

pub fn shutdown() {
    close();
    if APARTMENT.replace(false) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// The child exists immediately; environment and controller creation complete on the UI thread.
///
/// # Safety
/// The owner must be a live window on the calling UI thread.
pub unsafe fn open(owner: HWND, bounds: RECT) -> Result<(), String> {
    if active() {
        set_bounds(bounds);
        return Ok(());
    }
    let local = std::env::var_os("LOCALAPPDATA").ok_or("无法定位当前用户的 M HUB 配置目录")?;
    let profile = PathBuf::from(local).join("PeripheralDesk").join("MHub");
    std::fs::create_dir_all(&profile).map_err(|e| format!("无法创建 M HUB 配置目录：{e}"))?;
    let profile: Vec<u16> = profile.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        if !APARTMENT.get() {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| format!("无法初始化驱动中心：{e}"))?;
            APARTMENT.set(true);
        }
        let module = GetModuleHandleW(null());
        let class = wide("PeripheralDeskMHub");
        let wc = WNDCLASSW {
            hInstance: module,
            lpfnWndProc: Some(window_proc),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("正在打开迈从驱动中心…").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            bounds.left,
            bounds.top,
            (bounds.right - bounds.left).max(0),
            (bounds.bottom - bounds.top).max(0),
            owner,
            null_mut(),
            module,
            null(),
        );
        if hwnd.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let host = Rc::new(Host(hwnd));
        let session = Rc::downgrade(&host);
        HOST.set(Some(host));
        ERROR.take();
        if let Err(error) = begin_environment(session, &profile) {
            DestroyWindow(hwnd);
            return Err(startup_error(&error));
        }
    }
    Ok(())
}

fn startup_error(error: &windows::core::Error) -> String {
    format!("无法载入迈从驱动中心。请检查 Microsoft Edge WebView2 运行时后重新打开。\n{error}")
}

unsafe fn async_failure(hwnd: HWND, error: &windows::core::Error) {
    ERROR.set(Some(startup_error(error)));
    PostMessageW(hwnd, WM_CLOSE, 0, 0);
}

unsafe fn begin_environment(session: Weak<Host>, profile: &[u16]) -> windows::core::Result<()> {
    let completion =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            if let Some(host) = session.upgrade() {
                let result = result
                    .and_then(|()| env.ok_or_else(|| E_POINTER.into()))
                    .and_then(|env| begin_controller(&host, env));
                if let Err(error) = result {
                    async_failure(host.0, &error);
                }
            }
            Ok(())
        }));
    CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        PCWSTR(profile.as_ptr()),
        None,
        &completion,
    )
}

unsafe fn begin_controller(
    host: &Rc<Host>,
    env: ICoreWebView2Environment,
) -> windows::core::Result<()> {
    let session = Rc::downgrade(host);
    let environment = env.clone();
    let completion =
        CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, view| {
            let Some(host) = session.upgrade() else {
                if let Some(controller) = view {
                    let _ = controller.Close();
                }
                return Ok(());
            };
            let result = result
                .and_then(|()| view.ok_or_else(|| E_POINTER.into()))
                .and_then(|controller| attach(host.0, &environment, controller));
            if let Err(error) = result {
                async_failure(host.0, &error);
            }
            Ok(())
        }));
    env.CreateCoreWebView2Controller(ComWindow(host.0), &completion)
}

unsafe fn attach(
    hwnd: HWND,
    env: &ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
) -> windows::core::Result<()> {
    let web = controller.CoreWebView2()?;
    resources::attach(hwnd, &web, env)?;
    let settings = web.Settings()?;
    settings.SetAreHostObjectsAllowed(false)?;
    settings.SetIsWebMessageEnabled(false)?;
    let mut token = 0;
    web.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |sender, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if let Some(command) = setup_command(&uri) {
                    args.SetCancel(true)?;
                    if let Some(web) = sender {
                        let mut source = PWSTR::null();
                        web.Source(&mut source)?;
                        let mut clicked = windows::core::BOOL::default();
                        args.IsUserInitiated(&mut clicked)?;
                        if take_pwstr(source) == SETUP_PAGE && clicked.as_bool() {
                            PostMessageW(hwnd, WM_COMMAND, command, 0);
                        }
                    }
                } else if !uri.starts_with(workspace::LOCAL_SITE)
                    && uri != SETUP_PAGE
                    && uri != "about:blank"
                {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_FrameNavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if !uri.starts_with(workspace::LOCAL_SITE)
                    && !matches!(uri.as_str(), "about:blank" | "about:srcdoc")
                    && !uri
                        .strip_prefix("blob:")
                        .is_some_and(|p| p.starts_with(workspace::LOCAL_SITE))
                {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_PermissionRequested(
        &PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if uri != workspace::LOCAL_SITE.trim_end_matches('/')
                    && !uri.starts_with(workspace::LOCAL_SITE)
                {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_NewWindowRequested(
        &NewWindowRequestedEventHandler::create(Box::new(|browser, args| {
            if let (Some(browser), Some(args)) = (browser, args) {
                args.SetHandled(true)?;
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if uri.starts_with(workspace::LOCAL_SITE) {
                    browser.Navigate(PCWSTR(wide(&uri).as_ptr()))?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
            if let Some(args) = args {
                let mut success = windows::core::BOOL::default();
                args.IsSuccess(&mut success)?;
                if !success.as_bool() {
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                    args.WebErrorStatus(&mut status)?;
                    if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                        return Ok(());
                    }
                }
                let title = if success.as_bool() {
                    if let Some(web) = sender {
                        let mut uri = PWSTR::null();
                        web.Source(&mut uri)?;
                        if take_pwstr(uri).starts_with(workspace::LOCAL_SITE) {
                            let _ = menu::apply_style(&web);
                            menu::load_user_module(&web)?;
                        }
                    }
                    TITLE
                } else {
                    "本地驱动中心未载入 · 请在驱动中心菜单选择或安装资源"
                };
                SetWindowTextW(hwnd, wide(title).as_ptr());
            }
            Ok(())
        })),
        &mut token,
    )?;
    resize(hwnd, &controller)?;
    controller.SetZoomFactor(menu::saved_zoom())?;
    controller.SetIsVisible(true)?;
    menu::open_initial(hwnd, &web)?;
    controller.add_MoveFocusRequested(
        &MoveFocusRequestedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut reason = COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
                args.Reason(&mut reason)?;
                let back = reason == COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS;
                let next = GetNextDlgTabItem(GetParent(hwnd), hwnd, i32::from(back));
                if !next.is_null() && next != hwnd {
                    args.SetHandled(true)?;
                    SetFocus(next);
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd {
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    }
    CONTROLLER.set(Some(controller));
    Ok(())
}

fn setup_command(uri: &str) -> Option<usize> {
    match uri.strip_prefix(workspace::LOCAL_SITE)? {
        "_peripheral_desk/setup/install" => Some(menu::INSTALL),
        "_peripheral_desk/setup/choose" => Some(menu::SELECT_COPY),
        _ => None,
    }
}

unsafe fn resize(hwnd: HWND, controller: &ICoreWebView2Controller) -> windows::core::Result<()> {
    let mut rect = RECT::default();
    GetClientRect(hwnd, &mut rect);
    controller.SetBounds(ComRect {
        left: 0,
        top: 0,
        right: rect.right,
        bottom: rect.bottom,
    })
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        SHOW_MENU => {
            menu::popup(hwnd, w as i32, l as i32);
            0
        }
        WM_INITMENUPOPUP if (l >> 16) == 0 => {
            menu::update_menu(w as HMENU);
            0
        }
        WM_COMMAND if l == 0 => {
            menu::report(hwnd, menu::command(hwnd, w & 0xffff));
            0
        }
        menu::BUILTIN_READY => {
            let result = menu::builtin_ready();
            if result.is_err() {
                SetWindowTextW(hwnd, wide(TITLE).as_ptr());
            }
            menu::report(hwnd, result);
            0
        }
        WM_SIZE | WM_SETFOCUS | WM_MOVE => {
            if let Some(controller) = CONTROLLER.with_borrow(Clone::clone) {
                let _ = match msg {
                    WM_SIZE => resize(hwnd, &controller),
                    WM_SETFOCUS => controller.MoveFocus(if GetKeyState(VK_SHIFT as i32) < 0 {
                        COREWEBVIEW2_MOVE_FOCUS_REASON_PREVIOUS
                    } else {
                        COREWEBVIEW2_MOVE_FOCUS_REASON_NEXT
                    }),
                    _ => controller.NotifyParentWindowPositionChanged(),
                };
            }
            0
        }
        WM_DESTROY => {
            HOST.take();
            menu::close();
            resources::close();
            if let Some(controller) = CONTROLLER.take() {
                let _ = controller.Close();
            }
            PostMessageW(GetParent(hwnd), CLOSED_MESSAGE, 0, 0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
