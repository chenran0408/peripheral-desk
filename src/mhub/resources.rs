//! The vendor frontend runs against the files in the selected local edition.
use super::*;
use std::{fs, path::Path};
use webview2_com::WebResourceRequestedEventHandler;
use windows::{core::Interface, Win32::UI::Shell::SHCreateMemStream};

const CDN: &str = "https://cdn.mchose.com.cn/";
thread_local! {
    static ROOT: RefCell<Option<PathBuf>> = const {RefCell::new(None)};
}

pub(super) fn set_root(path: &Path) {
    ROOT.set(Some(path.to_path_buf()));
}
pub(super) fn close() {
    ROOT.take();
}

// The original frontend constructs these CDN addresses itself. Serve their
// published contents from the selected local snapshot, including inside workers.
fn cdn_asset(uri: &str) -> Option<(&str, &'static str)> {
    let path = uri.strip_prefix(CDN)?.split(['?', '#']).next()?;
    if !path.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && !part.ends_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.%".contains(&b))
    }) {
        return None;
    }
    let mime = match path.rsplit_once('.')?.1 {
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "css" => "text/css",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => return None,
    };
    Some((path, mime))
}

pub(super) unsafe fn attach(
    _hwnd: HWND,
    web: &ICoreWebView2,
    env: &ICoreWebView2Environment,
) -> windows::core::Result<()> {
    let all_sources: ICoreWebView2_22 = web.cast()?;
    for pattern in ["http://*", "https://*"] {
        all_sources.AddWebResourceRequestedFilterWithRequestSourceKinds(
            PCWSTR(wide(pattern).as_ptr()),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )?;
    }
    let env = env.clone();
    let mut token = 0;
    web.add_WebResourceRequested(
        &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let req = args.Request()?;
            let mut raw = PWSTR::null();
            req.Uri(&mut raw)?;
            let uri = take_pwstr(raw);
            if uri.starts_with(workspace::LOCAL_SITE) {
                return Ok(());
            }
            req.Method(&mut raw)?;
            let get = take_pwstr(raw) == "GET";
            let file = if get && uri == super::SETUP_PAGE {
                Some((include_bytes!("install.html").to_vec(), "text/html; charset=utf-8"))
            } else if get {
                cdn_asset(&uri).and_then(|(path, mime)| ROOT.with_borrow(|dir| {
                    fs::read(dir.as_ref()?.join("vendor-cdn").join(path)).ok().map(|bytes| (bytes, mime))
                }))
            } else { None };
            let (code, reason, bytes, mime) = match file {
                Some((bytes, mime)) => (200, "OK", bytes, mime),
                None => (403, "Local resources only", Vec::new(), "text/plain"),
            };
            let stream = SHCreateMemStream(Some(&bytes));
            let headers = format!(
                "Content-Type: {mime}\r\nAccess-Control-Allow-Origin: {}\r\nCache-Control: no-store",
                workspace::LOCAL_SITE.trim_end_matches('/')
            );
            let response = env.CreateWebResourceResponse(
                stream.as_ref(), code, PCWSTR(wide(reason).as_ptr()), PCWSTR(wide(&headers).as_ptr()),
            )?;
            args.SetResponse(&response)
        })),
        &mut token,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vendor_urls_only_resolve_to_packaged_static_files() {
        let keymap = "configCenter/custom/keyboardConfig.json";
        assert_eq!(
            cdn_asset(&format!("{CDN}{keymap}?v=42")),
            Some((keymap, "application/json"))
        );
        assert!(
            cdn_asset("https://outside.test/configCenter/custom/keyboardConfig.json").is_none()
        );
        assert!(cdn_asset(&format!("{CDN}../api/settings.json")).is_none());
        assert!(cdn_asset(&format!("{CDN}custom/remote.js")).is_none());
    }
}
