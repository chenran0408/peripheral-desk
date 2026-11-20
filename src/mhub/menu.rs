use super::*;
use std::sync::atomic::AtomicBool;
use windows::core::Interface;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

const OFFICIAL: usize = 2101;
const INSTALL: usize = 2102;
const LOCAL: usize = 2103;
const DIRECTORY: usize = 2104;
const STYLE: usize = 2111;
const APPLY_STYLE: usize = 2112;
const ZOOM_SMALL: usize = 2121;
const ZOOM_NORMAL: usize = 2122;
const ZOOM_LARGE: usize = 2123;
const RELOAD: usize = 2131;
const SELECT_COPY: usize = 2132;
const COLOR_DEFAULT: usize = 2140;
const COLOR_CORAL: usize = 2141;
const COLOR_TEAL: usize = 2142;
pub(super) const INSTALLED: u32 = WM_APP + 71;
pub(super) const PROGRESS: u32 = WM_APP + 72;
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
thread_local! {
    static RESULT: RefCell<Option<mpsc::Receiver<Result<PathBuf,String>>>> = const {RefCell::new(None)};
}

pub(super) unsafe fn create(hwnd: HWND) {
    let bar = CreateMenu();
    for (name, items) in [
        (
            "工作区(&W)",
            &[
                (INSTALL, "下载或更新可编辑副本…"),
                (LOCAL, "打开本地副本"),
                (DIRECTORY, "打开网页文件夹"),
                (SELECT_COPY, "切换已保存的版本…"),
                (OFFICIAL, "返回官方网站"),
                (RELOAD, "刷新当前页面"),
            ][..],
        ),
        (
            "外观(&V)",
            &[
                (COLOR_DEFAULT, "使用官网配色"),
                (COLOR_CORAL, "珊瑚红强调色"),
                (COLOR_TEAL, "青绿强调色"),
                (STYLE, "编辑个人样式…"),
                (APPLY_STYLE, "重新应用样式"),
                (ZOOM_SMALL, "缩放到 85%"),
                (ZOOM_NORMAL, "缩放到 100%"),
                (ZOOM_LARGE, "缩放到 125%"),
            ][..],
        ),
    ] {
        let submenu = CreatePopupMenu();
        for (id, label) in items {
            AppendMenuW(submenu, MF_STRING, *id, wide(label).as_ptr());
        }
        AppendMenuW(bar, MF_POPUP, submenu as usize, wide(name).as_ptr());
    }
    SetMenu(hwnd, bar);
}

fn zoom_file() -> Result<PathBuf, String> {
    Ok(workspace::root()?.join("zoom.txt"))
}

pub(super) fn saved_zoom() -> f64 {
    zoom_file()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|v| [85, 100, 125].contains(v))
        .map_or(1., |v| f64::from(v) / 100.)
}

unsafe fn browser() -> Result<ICoreWebView2, String> {
    CONTROLLER.with_borrow(|c| {
        c.as_ref()
            .ok_or("网页正在启动".to_owned())?
            .CoreWebView2()
            .map_err(|e| e.to_string())
    })
}

pub(super) unsafe fn apply_style(web: &ICoreWebView2) -> Result<(), String> {
    let path = workspace::root()?.join("custom.css");
    let css = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        other => other.map_err(|e| e.to_string())?,
    };
    let accent = match std::fs::read_to_string(workspace::root()?.join("accent.txt")).as_deref() {
        Ok("coral") => Some("#df635e"),
        Ok("teal") => Some("#279b91"),
        _ => None,
    };
    let sheet = if let Some(color) = accent {
        format!(":root:root,body[class]{{--du-accent-blue:{color};--el-color-primary:{color};--du-glow-hover-color:{color};}}\n{css}")
    } else {
        css
    };
    let css = serde_json::to_string(&sheet).map_err(|e| e.to_string())?;
    let js = format!("(()=>{{let s=document.getElementById('peripheral-desk-user-style');if(!s){{s=document.createElement('style');s.id='peripheral-desk-user-style';document.head.append(s)}}s.textContent={css};}})()");
    web.ExecuteScript(PCWSTR(wide(&js).as_ptr()), None)
        .map_err(|e| e.to_string())
}

unsafe fn open_path(path: &std::path::Path, edit: bool) -> Result<(), String> {
    let result = if edit {
        // Open the local stylesheet as text; a .css file association can point to a browser.
        let system = std::env::var_os("SystemRoot").ok_or("找不到系统编辑器")?;
        let editor = PathBuf::from(system).join("System32").join("notepad.exe");
        std::process::Command::new(editor)
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    } else {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            workspace::local_wide(path).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        return Err("无法打开工作区目录".into());
    }
    Ok(())
}

unsafe fn navigate_local(web: &ICoreWebView2, path: &std::path::Path) -> Result<(), String> {
    let web3: ICoreWebView2_3 = web.cast().map_err(|e| e.to_string())?;
    web3.SetVirtualHostNameToFolderMapping(
        PCWSTR(wide("mhub.peripheraldesk.test").as_ptr()),
        PCWSTR(workspace::local_wide(path).as_ptr()),
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY,
    )
    .map_err(|e| e.to_string())?;
    resources::set_root(path);
    web.Navigate(PCWSTR(
        wide(&format!("{}index.html", workspace::LOCAL_SITE)).as_ptr(),
    ))
    .map_err(|e| e.to_string())
}

pub(super) unsafe fn command(hwnd: HWND, id: usize) -> Result<(), String> {
    let web = browser()?;
    match id {
        OFFICIAL => {
            web.Navigate(PCWSTR(wide(SITE).as_ptr()))
                .map_err(|e| e.to_string())?;
            remember_mode(false)
        }
        RELOAD => web.Reload().map_err(|e| e.to_string()),
        INSTALL => {
            if DOWNLOADING.swap(true, Ordering::AcqRel) {
                return Err("已有工作区下载正在进行".into());
            }
            SetWindowTextW(hwnd, wide("正在准备可编辑工作区…").as_ptr());
            let window = hwnd as usize;
            let (tx, rx) = mpsc::channel();
            RESULT.set(Some(rx));
            thread::spawn(move || {
                let result = workspace::install(|n, total| {
                    PostMessageW(window as HWND, PROGRESS, n, total as isize);
                });
                DOWNLOADING.store(false, Ordering::Release);
                if tx.send(result).is_ok() {
                    PostMessageW(window as HWND, INSTALLED, 0, 0);
                }
            });
            Ok(())
        }
        LOCAL => open_local(&web, &workspace::selected()?),
        SELECT_COPY => {
            if let Some(path) = choose_previous(hwnd)? {
                open_local(&web, &path)?;
            }
            Ok(())
        }
        COLOR_DEFAULT | COLOR_CORAL | COLOR_TEAL => {
            let color = match id {
                COLOR_CORAL => "coral",
                COLOR_TEAL => "teal",
                _ => "default",
            };
            crate::storage::atomic_write(&workspace::root()?.join("accent.txt"), color.as_bytes())?;
            apply_style(&web)
        }
        DIRECTORY => open_path(&workspace::selected()?, false),
        STYLE => open_path(&workspace::styles_path()?, true),
        APPLY_STYLE => apply_style(&web),
        ZOOM_SMALL | ZOOM_NORMAL | ZOOM_LARGE => {
            let zoom = match id {
                ZOOM_SMALL => 85,
                ZOOM_LARGE => 125,
                _ => 100,
            };
            CONTROLLER.with_borrow(|c| {
                c.as_ref()
                    .ok_or("网页正在启动".to_string())?
                    .SetZoomFactor(f64::from(zoom) / 100.)
                    .map_err(|e| e.to_string())
            })?;
            crate::storage::atomic_write(&zoom_file()?, zoom.to_string().as_bytes())
        }
        _ => Ok(()),
    }
}

pub(super) unsafe fn installed(hwnd: HWND) -> Result<(), String> {
    let result = RESULT.take().and_then(|rx| rx.try_recv().ok());
    if let Some(result) = result {
        let path = result?;
        if MessageBoxW(
            hwnd,
            wide("网页已保存。现在打开本地副本？\n你可以从“工作区”菜单找到并修改网页文件。")
                .as_ptr(),
            wide("M HUB 可编辑工作区").as_ptr(),
            MB_YESNO | MB_ICONINFORMATION,
        ) == IDYES
        {
            open_local(&browser()?, &path)?;
        }
    }
    Ok(())
}

pub(super) unsafe fn report(hwnd: HWND, result: Result<(), String>) {
    if let Err(e) = result {
        MessageBoxW(
            hwnd,
            wide(&e).as_ptr(),
            wide("M HUB 工作区").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub(super) fn has_download() -> bool {
    RESULT.with_borrow(|r| r.is_some())
}

fn remember_mode(local: bool) -> Result<(), String> {
    crate::storage::atomic_write(
        &workspace::root()?.join("mode.txt"),
        if local { b"local" } else { b"official" },
    )
}

unsafe fn open_local(web: &ICoreWebView2, path: &std::path::Path) -> Result<(), String> {
    navigate_local(web, path)?;
    let build = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| workspace::build_name(s))
        .ok_or("网页副本的版本目录无效")?;
    crate::storage::atomic_write(&workspace::root()?.join("current.txt"), build.as_bytes())?;
    remember_mode(true)
}

pub(super) unsafe fn open_initial(web: &ICoreWebView2) -> windows::core::Result<()> {
    if let Ok(root) = workspace::root() {
        if std::fs::read_to_string(root.join("mode.txt"))
            .as_deref()
            .ok()
            == Some("local")
        {
            if let Ok(path) = workspace::selected() {
                return navigate_local(web, &path).map_err(|e| {
                    windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), &e)
                });
            }
        }
    }
    web.Navigate(PCWSTR(wide(SITE).as_ptr()))
}

unsafe fn choose_previous(hwnd: HWND) -> Result<Option<PathBuf>, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let root = workspace::root()?;
    let mut file = vec![0u16; 32768];
    let filter = wide("网页入口（index.html）\0index.html\0");
    let title = wide("选择已保存版本中的 index.html");
    let directory = workspace::local_wide(&root);
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrInitialDir: directory.as_ptr(),
        lpstrFilter: filter.as_ptr(),
        Flags: OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if GetOpenFileNameW(&mut dialog) == 0 {
        let err = CommDlgExtendedError();
        return if err == 0 {
            Ok(None)
        } else {
            Err(format!("无法选择网页文件（{err}）"))
        };
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(
        &file[..file.iter().position(|&n| n == 0).unwrap_or(file.len())],
    ));
    let dir = path.parent().ok_or("请选择工作区内一个版本的 index.html")?;
    if path.file_name().and_then(|s| s.to_str()) != Some("index.html")
        || dir.parent() != Some(root.as_path())
        || !dir
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(workspace::build_name)
    {
        return Err("请选择 MHubWorkspace 中已保存版本的 index.html".into());
    }
    Ok(Some(dir.to_owned()))
}
