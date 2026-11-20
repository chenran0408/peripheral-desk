use super::*;
use windows::core::Interface;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

const OPEN_BUILTIN: usize = 2105;
const UPDATE_NOTICE: usize = 2106;
pub(super) const INSTALL: usize = 2102;
const LOCAL: usize = 2103;
const DIRECTORY: usize = 2104;
const STYLE: usize = 2111;
const SCRIPT: usize = 2113;
const APPLY_STYLE: usize = 2112;
const ZOOM_SMALL: usize = 2121;
const ZOOM_NORMAL: usize = 2122;
const ZOOM_LARGE: usize = 2123;
const RELOAD: usize = 2131;
pub(super) const SELECT_COPY: usize = 2132;
const COLOR_DEFAULT: usize = 2140;
const COLOR_CORAL: usize = 2141;
const COLOR_TEAL: usize = 2142;
pub(super) const BUILTIN_READY: u32 = WM_APP + 75;
type Prepared = Result<(PathBuf, &'static str), String>;
thread_local! {
    static BUILTIN_RESULT: RefCell<Option<mpsc::Receiver<Prepared>>> = const {RefCell::new(None)};
}

pub(super) fn close() {
    BUILTIN_RESULT.take();
}

pub(super) unsafe fn popup(hwnd: HWND, x: i32, y: i32) {
    let bar = CreatePopupMenu();
    for (name, items) in [
        (
            "驱动中心(&W)",
            &[
                (OPEN_BUILTIN, "使用随软件安装的版本"),
                (INSTALL, "安装或更新驱动中心…"),
                (LOCAL, "使用当前本地版本"),
                (DIRECTORY, "打开驱动中心文件夹"),
                (SELECT_COPY, "切换本地版本…"),
                (RELOAD, "刷新当前页面"),
            ][..],
        ),
        (
            "外观(&V)",
            &[
                (COLOR_DEFAULT, "使用默认配色"),
                (COLOR_CORAL, "珊瑚红强调色"),
                (COLOR_TEAL, "青绿强调色"),
                (STYLE, "编辑个人样式…"),
                (SCRIPT, "编辑本地交互脚本…"),
                (APPLY_STYLE, "重新应用样式"),
                (ZOOM_SMALL, "缩放到 85%"),
                (ZOOM_NORMAL, "缩放到 100%"),
                (ZOOM_LARGE, "缩放到 125%"),
            ][..],
        ),
    ] {
        let submenu = CreatePopupMenu();
        for (id, label) in items {
            if *id == OPEN_BUILTIN && builtin::package().is_none() {
                continue;
            }
            AppendMenuW(submenu, MF_STRING, *id, wide(label).as_ptr());
        }
        AppendMenuW(bar, MF_POPUP, submenu as usize, wide(name).as_ptr());
    }
    if workspace::selected()
        .is_ok_and(|p| builtin::installed_version(&p).as_deref() != Some(env!("CARGO_PKG_VERSION")))
    {
        AppendMenuW(
            bar,
            MF_STRING,
            UPDATE_NOTICE,
            wide(concat!("获取 ", env!("CARGO_PKG_VERSION"), " 版驱动中心…")).as_ptr(),
        );
    }
    let id = TrackPopupMenu(bar, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, 0, hwnd, null());
    DestroyMenu(bar);
    if id > 0 {
        PostMessageW(hwnd, WM_COMMAND, id as usize, 0);
    }
}

pub(super) unsafe fn update_menu(menu: HMENU) {
    let has_workspace = workspace::selected().is_ok();
    let busy = BUILTIN_RESULT.with_borrow(|slot| slot.is_some());
    for (id, enabled) in [
        (LOCAL, has_workspace),
        (DIRECTORY, has_workspace),
        (INSTALL, !busy),
        (OPEN_BUILTIN, !busy),
    ] {
        EnableMenuItem(
            menu,
            id as u32,
            MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
        );
    }
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

unsafe fn navigate_local(
    web: &ICoreWebView2,
    path: &std::path::Path,
    managed: bool,
) -> Result<(), String> {
    workspace::prepare_user_hooks(path)?;
    let web3: ICoreWebView2_3 = web.cast().map_err(|e| e.to_string())?;
    web3.SetVirtualHostNameToFolderMapping(
        PCWSTR(wide("mhub.peripheraldesk.test").as_ptr()),
        PCWSTR(workspace::local_wide(path).as_ptr()),
        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY,
    )
    .map_err(|e| e.to_string())?;
    let native = managed
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(builtin::folder_name);
    // The mapped HTTPS origin resolves these files in WebView2's renderer.
    // Prepare the per-launch context before the vendor's first getDevices().
    let generated = path.join("_peripheral_desk").join("connection.js");
    crate::storage::atomic_write(&generated, transport::script(native).as_bytes())?;
    transport::select_edition(native);
    resources::set_root(path);
    web.Navigate(PCWSTR(
        wide(&format!("{}index.html", workspace::LOCAL_SITE)).as_ptr(),
    ))
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) unsafe fn command(hwnd: HWND, id: usize) -> Result<(), String> {
    if matches!(
        id,
        OPEN_BUILTIN | UPDATE_NOTICE | INSTALL | RELOAD | LOCAL | SELECT_COPY
    ) && writes::pending()
    {
        return Err("G75 Pro 正在确认板载键位，请在写完后切换或重载工作区。".into());
    }
    let web = browser()?;
    match id {
        OPEN_BUILTIN => prepare(
            hwnd,
            Some(builtin::package().ok_or("此版本未附带驱动中心资源，请选择安装或更新")?),
        ),
        INSTALL | UPDATE_NOTICE => {
            // A full install already contains the matching release and also works offline.
            prepare(
                hwnd,
                builtin::package().filter(|source| builtin::is_current(source)),
            )
        }
        RELOAD => {
            let mut uri = PWSTR::null();
            web.Source(&mut uri).map_err(|e| e.to_string())?;
            if take_pwstr(uri).starts_with(workspace::LOCAL_SITE) {
                workspace::prepare_user_hooks(&workspace::selected()?)?;
                web.Reload()
            } else {
                show_install(&web)
            }
            .map_err(|e| e.to_string())
        }
        LOCAL => open_local(&web, &workspace::selected()?, "local"),
        SELECT_COPY => {
            if let Some(path) = choose_previous(hwnd)? {
                open_local(&web, &path, "local")?;
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
        SCRIPT => open_path(&workspace::script_path()?, true),
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

pub(super) unsafe fn report(hwnd: HWND, result: Result<(), String>) {
    if let Err(e) = result {
        MessageBoxW(
            GetAncestor(hwnd, GA_ROOT),
            wide(&e).as_ptr(),
            wide(TITLE).as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn remember_mode(mode: &str) -> Result<(), String> {
    crate::storage::atomic_write(&workspace::root()?.join("mode.txt"), mode.as_bytes())
}

unsafe fn open_local(
    web: &ICoreWebView2,
    path: &std::path::Path,
    mode: &str,
) -> Result<(), String> {
    BUILTIN_RESULT.take();
    navigate_local(web, path, matches!(mode, "builtin" | "managed"))?;
    let build = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| workspace::workspace_name(s))
        .ok_or("网页副本的版本目录无效")?;
    crate::storage::atomic_write(&workspace::root()?.join("current.txt"), build.as_bytes())?;
    remember_mode(mode)
}

unsafe fn prepare(hwnd: HWND, package: Option<PathBuf>) -> Result<(), String> {
    if BUILTIN_RESULT.with_borrow(|r| r.is_some()) {
        return Ok(());
    }
    let root = workspace::root()?;
    let (tx, rx) = mpsc::channel();
    let window = hwnd as usize;
    let progress = if package.is_some() {
        "正在打开内置驱动中心…"
    } else {
        "正在安装或更新迈从驱动中心…"
    };
    thread::Builder::new()
        .name("迈从驱动中心".into())
        .spawn(move || {
            let result = match package {
                Some(dir) => builtin::prepare(&dir, &root).map(|p| (p, "builtin")),
                None => release_resources::install(&root).map(|p| (p, "managed")),
            };
            if tx.send(result).is_ok() {
                PostMessageW(window as HWND, BUILTIN_READY, 0, 0);
            }
        })
        .map_err(|e| e.to_string())?;
    BUILTIN_RESULT.set(Some(rx));
    SetWindowTextW(hwnd, wide(progress).as_ptr());
    Ok(())
}

unsafe fn show_install(web: &ICoreWebView2) -> windows::core::Result<()> {
    web.Navigate(PCWSTR(wide(SETUP_PAGE).as_ptr()))
}

pub(super) unsafe fn builtin_ready() -> Result<(), String> {
    let result = BUILTIN_RESULT.with_borrow(|r| r.as_ref().and_then(|rx| rx.try_recv().ok()));
    if let Some(result) = result {
        BUILTIN_RESULT.take();
        let web = browser()?;
        if let Err(error) = result.and_then(|(path, mode)| open_local(&web, &path, mode)) {
            let mut source = PWSTR::null();
            web.Source(&mut source).map_err(|e| e.to_string())?;
            if take_pwstr(source).starts_with(workspace::LOCAL_SITE) {
                return Err(error);
            }
            if let Ok(previous) = workspace::selected() {
                match open_local(&web, &previous, "local") {
                    Ok(()) => return Err(format!("{error}。已恢复之前的本地驱动中心。")),
                    Err(restore) => {
                        show_install(&web).map_err(|e| e.to_string())?;
                        return Err(format!("{error}。之前的版本也未能打开：{restore}"));
                    }
                }
            }
            show_install(&web).map_err(|e| e.to_string())?;
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Initial {
    Install,
    Local(PathBuf),
    Bundled(PathBuf),
}

fn initial_choice(
    mode: Option<&str>,
    bundle: Option<PathBuf>,
    selected: Option<PathBuf>,
) -> Initial {
    let current_bundle = bundle.as_deref().is_some_and(builtin::is_current);
    match (mode, bundle, selected) {
        (Some("local" | "official") | None, _, Some(p)) => Initial::Local(p),
        (_, Some(p), _) if current_bundle => Initial::Bundled(p),
        (_, _, Some(p)) => Initial::Local(p),
        (_, Some(p), None) => Initial::Bundled(p),
        _ => Initial::Install,
    }
}

pub(super) unsafe fn open_initial(hwnd: HWND, web: &ICoreWebView2) -> windows::core::Result<()> {
    let mode = workspace::root()
        .ok()
        .and_then(|root| std::fs::read_to_string(root.join("mode.txt")).ok());
    let choice = initial_choice(
        mode.as_deref(),
        builtin::package(),
        workspace::selected().ok(),
    );
    let result = match choice {
        Initial::Local(path) => navigate_local(
            web,
            &path,
            matches!(mode.as_deref(), Some("builtin" | "managed")),
        )
        .and_then(|()| {
            if mode.as_deref() == Some("official") {
                remember_mode("local")
            } else {
                Ok(())
            }
        }),
        Initial::Bundled(path) => prepare(hwnd, Some(path)),
        Initial::Install => return show_install(web),
    };
    result.map_err(|e| windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), e))
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
        hwndOwner: GetAncestor(hwnd, GA_ROOT),
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
            .is_some_and(workspace::workspace_name)
    {
        return Err("请选择 MHubWorkspace 中已保存版本的 index.html".into());
    }
    Ok(Some(dir.to_owned()))
}

pub(super) unsafe fn load_user_module(web: &ICoreWebView2) -> windows::core::Result<()> {
    if !workspace::root().is_ok_and(|p| p.join("personal.js").is_file()) {
        return Ok(());
    }
    // The desktop only adds a fixed same-origin URL. The editable code stays in the local site.
    let code = r#"(() => {
        if(location.origin!=='https://mhub.peripheraldesk.test'||document.querySelector('script[data-peripheral-desk]')) return;
        const script=document.createElement('script');
        script.type='module'; script.dataset.peripheralDesk='personal';
        script.src='/_peripheral_desk/personal.js?v='+Date.now(); document.head.append(script);
    })()"#;
    web.ExecuteScript(PCWSTR(wide(code).as_ptr()), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_start_migrates_official_and_keeps_selected_workspace() {
        let bundle = PathBuf::from("pack");
        let edited = PathBuf::from("edited");
        assert_eq!(
            initial_choice(None, Some(bundle.clone()), None),
            Initial::Bundled(bundle.clone())
        );
        assert_eq!(
            initial_choice(Some("official"), Some(bundle.clone()), Some(edited.clone())),
            Initial::Local(edited.clone())
        );
        assert_eq!(
            initial_choice(Some("local"), Some(bundle), Some(edited.clone())),
            Initial::Local(edited.clone())
        );
        assert_eq!(
            initial_choice(Some("builtin"), None, Some(edited.clone())),
            Initial::Local(edited)
        );
        assert_eq!(
            initial_choice(Some("official"), Some(PathBuf::from("pack")), None),
            Initial::Bundled(PathBuf::from("pack"))
        );
        assert_eq!(
            initial_choice(Some("official"), None, None),
            Initial::Install
        );
        assert_eq!(initial_choice(None, None, None), Initial::Install);
    }
    #[test]
    fn core_update_keeps_the_working_copy_until_current_resources_are_available() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pkg = std::env::temp_dir().join(format!("pd-hub-menu-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&pkg).unwrap();
        let selected = PathBuf::from("previous-workspace");
        let route = |mode| initial_choice(Some(mode), Some(pkg.clone()), Some(selected.clone()));
        let mut metadata =
            serde_json::json!({"schema":1,"build":"main-Abc12_XY.js","sha256":"ab".repeat(32)});
        for legacy in [true, false] {
            if !legacy {
                metadata["app_version"] = "0.4.11-preview.12".into();
            }
            fs::write(pkg.join("manifest.json"), metadata.to_string()).unwrap();
            assert_eq!(route("managed"), Initial::Local(selected.clone()));
            assert_eq!(route("builtin"), Initial::Local(selected.clone()));
        }
        metadata["app_version"] = env!("CARGO_PKG_VERSION").into();
        fs::write(pkg.join("manifest.json"), metadata.to_string()).unwrap();
        assert_eq!(route("managed"), Initial::Bundled(pkg.clone()));
        assert_eq!(route("builtin"), Initial::Bundled(pkg.clone()));
        assert_eq!(route("local"), Initial::Local(selected));
        fs::remove_dir_all(pkg).unwrap();
    }
}
