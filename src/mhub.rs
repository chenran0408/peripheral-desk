//! Official online M HUB in a dedicated WebView2 window and COM apartment.
use crate::hid::wide;
use std::{
    cell::RefCell,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
use webview2_com::{
    take_pwstr, wait_with_pump, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler,
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
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

const SITE: &str = "https://www.mchose.com.cn/";
const TITLE: &str = "迈从 M HUB · 官方网页驱动";
const STARTING: usize = 1;
static WINDOW: AtomicUsize = AtomicUsize::new(0);
pub const CLOSED_MESSAGE: u32 = WM_APP + 0x4d;

thread_local! {
    static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
}

/// While the official page owns the session, local MCHOSE polling and controls are paused.
pub fn active() -> bool {
    WINDOW.load(Ordering::Acquire) != 0
}

/// Returns immediately. The owner receives CLOSED_MESSAGE after the controller has closed.
pub fn open(owner: HWND) -> Result<(), String> {
    if let Err(current) = WINDOW.compare_exchange(0, STARTING, Ordering::AcqRel, Ordering::Acquire)
    {
        if current != STARTING {
            let window = current as HWND;
            unsafe {
                if IsIconic(window) != 0 {
                    ShowWindowAsync(window, SW_RESTORE);
                }
                SetForegroundWindow(window);
            }
        }
        return Ok(());
    }
    let owner_id = owner as usize;
    thread::Builder::new()
        .name("M HUB".into())
        .spawn(move || {
            let result = run(owner_id as HWND);
            WINDOW.store(0, Ordering::Release);
            unsafe {
                if owner_id != 0 {
                    PostMessageW(owner_id as HWND, CLOSED_MESSAGE, 0, 0);
                }
                if let Err(error) = result {
                    MessageBoxW(
                        null_mut(),
                        wide(&error).as_ptr(),
                        wide(TITLE).as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|error| {
            WINDOW.store(0, Ordering::Release);
            format!("无法启动 M HUB 窗口：{error}")
        })
}

fn run(owner: HWND) -> Result<(), String> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(|e| e.to_string())?;
    let result = run_window(owner);
    unsafe { CoUninitialize() };
    result
}

fn run_window(owner: HWND) -> Result<(), String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("无法定位当前用户的 M HUB 配置目录")?;
    let profile = PathBuf::from(local).join("PeripheralDesk").join("MHub");
    std::fs::create_dir_all(&profile).map_err(|e| format!("无法创建 M HUB 配置目录：{e}"))?;
    let profile: Vec<u16> = profile.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = GetModuleHandleW(null());
        let class = wide("PeripheralDeskMHub");
        let wc = WNDCLASSW {
            hInstance: module,
            lpfnWndProc: Some(window_proc),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: windows_sys::Win32::Graphics::Gdi::GetSysColorBrush(
                windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW,
            ),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (x, y, width, height) = if GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0
        {
            let rect = monitor.rcWork;
            let dx = (rect.right - rect.left) / 20;
            let dy = (rect.bottom - rect.top) / 20;
            (
                rect.left + dx,
                rect.top + dy,
                rect.right - rect.left - 2 * dx,
                rect.bottom - rect.top - 2 * dy,
            )
        } else {
            (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT)
        };
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("正在打开迈从 M HUB…").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VISIBLE,
            x,
            y,
            width,
            height,
            null_mut(),
            null_mut(),
            module,
            null(),
        );
        if hwnd.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        WINDOW.store(hwnd as usize, Ordering::Release);
        if let Err(error) = attach(hwnd, &profile) {
            // Closing the window during startup cancels the pending WebView2 callback.
            if IsWindow(hwnd) == 0 {
                return Ok(());
            }
            DestroyWindow(hwnd);
            return Err(format!(
                "无法载入迈从网页驱动。请检查 Microsoft Edge WebView2 运行时后重新打开。\n{error}"
            ));
        }
        let mut msg = MSG::default();
        loop {
            match GetMessageW(&mut msg, null_mut(), 0, 0) {
                -1 => return Err(std::io::Error::last_os_error().to_string()),
                0 => return Ok(()),
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

unsafe fn attach(hwnd: HWND, profile: &[u16]) -> webview2_com::Result<()> {
    let (tx, rx) = mpsc::channel();
    let completion =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            let _ = tx.send(
                result.and_then(|()| env.ok_or_else(|| windows::core::Error::from(E_POINTER))),
            );
            Ok(())
        }));
    CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        PCWSTR(profile.as_ptr()),
        None,
        &completion,
    )?;
    let env = wait_with_pump(rx)??;

    let (tx, rx) = mpsc::channel();
    let completion =
        CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, view| {
            let _ = tx.send(
                result.and_then(|()| view.ok_or_else(|| windows::core::Error::from(E_POINTER))),
            );
            Ok(())
        }));
    env.CreateCoreWebView2Controller(ComWindow(hwnd), &completion)?;
    let controller = wait_with_pump(rx)??;
    let web = controller.CoreWebView2()?;
    let settings = web.Settings()?;
    settings.SetAreHostObjectsAllowed(false)?;
    settings.SetIsWebMessageEnabled(false)?;
    let mut token = 0;
    web.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if !uri.starts_with(SITE) {
                    args.SetCancel(true)?;
                    let mut clicked = windows::core::BOOL::default();
                    args.IsUserInitiated(&mut clicked)?;
                    open_external_link(&uri, clicked.as_bool());
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
                if uri.starts_with(SITE) {
                    browser.Navigate(PCWSTR(wide(&uri).as_ptr()))?;
                } else {
                    let mut clicked = windows::core::BOOL::default();
                    args.IsUserInitiated(&mut clicked)?;
                    open_external_link(&uri, clicked.as_bool());
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
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
                    TITLE
                } else {
                    "M HUB 页面未载入 · 请检查网络后刷新"
                };
                SetWindowTextW(hwnd, wide(title).as_ptr());
            }
            Ok(())
        })),
        &mut token,
    )?;
    resize(hwnd, &controller)?;
    controller.SetIsVisible(true)?;
    web.Navigate(PCWSTR(wide(SITE).as_ptr()))?;
    let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
    CONTROLLER.set(Some(controller));
    Ok(())
}

unsafe fn open_external_link(uri: &str, user_initiated: bool) {
    if user_initiated && uri.starts_with("https://") {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide(uri).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
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
        WM_SIZE | WM_SETFOCUS | WM_MOVE => {
            if let Some(controller) = CONTROLLER.with_borrow(Clone::clone) {
                let _ = match msg {
                    WM_SIZE => resize(hwnd, &controller),
                    WM_SETFOCUS => {
                        controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
                    }
                    _ => controller.NotifyParentWindowPositionChanged(),
                };
            }
            0
        }
        WM_DESTROY => {
            if let Some(controller) = CONTROLLER.take() {
                let _ = controller.Close();
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
