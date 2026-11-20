//! Lazy, public vendor assets referenced at runtime by the editable frontend.
use super::*;
use std::{
    collections::HashMap,
    path::{Component, Path},
    sync::mpsc::Receiver,
};
use webview2_com::WebResourceRequestedEventHandler;
use windows::Win32::UI::Shell::SHCreateMemStream;

pub(super) const READY: u32 = WM_APP + 73;
struct Request {
    arguments: ICoreWebView2WebResourceRequestedEventArgs,
    environment: ICoreWebView2Environment,
    deferral: ICoreWebView2Deferral,
    mime: &'static str,
    data: Receiver<Result<Vec<u8>, String>>,
}
thread_local! {
    static PENDING: RefCell<HashMap<usize,Request>>=RefCell::new(HashMap::new());
    static ROOT: RefCell<Option<PathBuf>>=const {RefCell::new(None)};
}
static NEXT: AtomicUsize = AtomicUsize::new(1);

pub(super) fn set_root(path: &Path) {
    ROOT.set(Some(path.to_path_buf()));
}

fn public_path(uri: &str) -> Option<(&str, &str, &'static str)> {
    let resource = uri.strip_prefix(workspace::LOCAL_SITE)?;
    let path = resource.split(['?', '#']).next()?;
    if !["assets/", "cizhou/"].iter().any(|s| path.starts_with(s))
        || path.contains("//")
        || path.split('/').any(|s| {
            s == "."
                || s == ".."
                || s.ends_with('.')
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let mime = match path.rsplit_once('.')?.1 {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        _ => return None,
    };
    Some((path, resource, mime))
}

pub(super) unsafe fn attach(
    hwnd: HWND,
    web: &ICoreWebView2,
    env: &ICoreWebView2Environment,
) -> windows::core::Result<()> {
    let env = env.clone();
    for prefix in ["assets/", "cizhou/"] {
        web.AddWebResourceRequestedFilter(
            PCWSTR(wide(&format!("{}{prefix}*", workspace::LOCAL_SITE)).as_ptr()),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
    }
    let mut token = 0;
    web.add_WebResourceRequested(
        &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let req = args.Request()?;
            let mut raw = PWSTR::null();
            req.Method(&mut raw)?;
            if take_pwstr(raw) != "GET" {
                return Ok(());
            }
            req.Uri(&mut raw)?;
            let uri = take_pwstr(raw);
            let Some((path, resource, mime)) = public_path(&uri) else {
                return Ok(());
            };
            let Some(target) = ROOT.with_borrow(|p| p.as_ref().map(|dir| dir.join(path))) else {
                return Ok(());
            };
            if target.is_file() {
                return Ok(());
            }
            let resource = resource.to_owned();
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = mpsc::channel();
            let defer = args.GetDeferral()?;
            PENDING.with_borrow_mut(|r| {
                r.insert(
                    id,
                    Request {
                        arguments: args,
                        deferral: defer,
                        environment: env.clone(),
                        mime,
                        data: rx,
                    },
                )
            });
            let window = hwnd as usize;
            thread::spawn(move || {
                let result = workspace::fetch(&resource).and_then(|bytes| {
                    if !target.is_file() {
                        crate::storage::atomic_write(&target, &bytes)?;
                    }
                    Ok(bytes)
                });
                if tx.send(result).is_ok() {
                    PostMessageW(window as HWND, READY, id, 0);
                }
            });
            Ok(())
        })),
        &mut token,
    )
}

pub(super) unsafe fn ready(id: usize) {
    if let Some(req) = PENDING.with_borrow_mut(|r| r.remove(&id)) {
        if let Ok(result) = req.data.try_recv() {
            let (data, code, reason, mime) = match result {
                Ok(bytes) => (bytes, 200, "OK", req.mime),
                Err(e) => (
                    e.into_bytes(),
                    502,
                    "Resource unavailable",
                    "text/plain; charset=utf-8",
                ),
            };
            if let Some(stream) = SHCreateMemStream(Some(&data)) {
                if let Ok(response) = req.environment.CreateWebResourceResponse(
                    &stream,
                    code,
                    PCWSTR(wide(reason).as_ptr()),
                    PCWSTR(
                        wide(&format!("Content-Type: {mime}\r\nCache-Control: no-store")).as_ptr(),
                    ),
                ) {
                    let _ = req.arguments.SetResponse(&response);
                }
            }
        }
        let _ = req.deferral.Complete();
    }
}
pub(super) unsafe fn close() {
    for (_, r) in PENDING.take() {
        let _ = r.deferral.Complete();
    }
    ROOT.take();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_local_public_assets_use_the_vendor_asset_loader() {
        let site = workspace::LOCAL_SITE;
        assert_eq!(
            public_path(&format!("{site}cizhou/CZ_SHARED_DATA/manifest.json?v=4"))
                .unwrap()
                .0,
            "cizhou/CZ_SHARED_DATA/manifest.json"
        );
        for url in [
            "https://elsewhere.test/assets/ui.js",
            "https://mhub.peripheraldesk.test/api/profile.json",
            "https://mhub.peripheraldesk.test/assets/../api/user.json",
            "https://mhub.peripheraldesk.test/assets/%2e%2e/file.js",
        ] {
            assert!(public_path(url).is_none());
        }
    }
}
