//! Connect the bundled, local M HUB document to the app's device worker.
use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use webview2_com::{ContentLoadingEventHandler, WebMessageReceivedEventHandler};

pub(super) const READY: u32 = WM_APP + 0x58;
const CHANNEL: &str = "peripheral-desk-hid";

thread_local! {
    static MANAGED: Cell<bool> = const { Cell::new(false) };
    static DOCUMENT: Cell<bool> = const { Cell::new(false) };
    static PREFERRED: Cell<Option<&'static str>> = const { Cell::new(None) };
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

struct Session {
    requests: mpsc::Sender<Job>,
    bridge: Arc<native_hid::Bridge>,
    replies: Arc<Mailbox>,
}

enum Job {
    Request(Value),
    DevicesChanged,
}

struct Mailbox {
    window: usize,
    messages: Mutex<Vec<Value>>,
}

impl Mailbox {
    fn send(&self, value: Value) {
        let mut queue = self.messages.lock().unwrap();
        let wake = queue.is_empty();
        queue.push(value);
        if wake {
            unsafe {
                PostMessageW(self.window as HWND, READY, 0, 0);
            }
        }
    }
}

impl Session {
    fn start(hwnd: HWND) -> Result<Self, String> {
        let replies = Arc::new(Mailbox {
            window: hwnd as usize,
            messages: Mutex::default(),
        });
        let events = replies.clone();
        let bridge = Arc::new(native_hid::Bridge::new(move |mut event: Value| {
            event["channel"] = Value::String(CHANNEL.into());
            events.send(event);
        }));
        let worker = bridge.clone();
        let outgoing = replies.clone();
        let (requests, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("迈从设备连接".into())
            .spawn(move || {
                for job in rx {
                    let request = match job {
                        Job::DevicesChanged => {
                            let _ = worker.refresh();
                            continue;
                        }
                        Job::Request(request) => request,
                    };
                    let id = &request["id"];
                    let reply = match worker.request(&request) {
                        Ok(value) => json!({"channel":CHANNEL,"id":id,"ok":true,"value":value}),
                        Err(error) => json!({"channel":CHANNEL,"id":id,"ok":false,"error":error}),
                    };
                    outgoing.send(reply);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            requests,
            bridge,
            replies,
        })
    }
}

fn release() {
    if let Some(session) = SESSION.take() {
        session.bridge.shutdown();
    }
}

pub fn devices_changed() {
    SESSION.with_borrow(|s| {
        if let Some(s) = s {
            let _ = s.requests.send(Job::DevicesChanged);
        }
    });
}

pub(super) fn prefer(model: Option<&'static str>) {
    PREFERRED.set(model);
}

pub(super) fn select_edition(managed: bool) {
    MANAGED.set(managed);
    DOCUMENT.set(false);
    release();
}

pub(super) fn script(managed: bool) -> String {
    let preference = serde_json::to_string(&PREFERRED.get()).unwrap();
    let source = if managed {
        include_str!("hub/native-transport.js")
    } else {
        ""
    };
    format!("window.__peripheralDeskPreferredModel={preference};\n{source}")
}

unsafe fn local_document(web: &ICoreWebView2) -> windows::core::Result<bool> {
    let mut uri = PWSTR::null();
    web.Source(&mut uri)?;
    Ok(take_pwstr(uri).starts_with(workspace::LOCAL_SITE))
}

unsafe fn post(web: &ICoreWebView2, reply: &Value) -> windows::core::Result<()> {
    web.PostWebMessageAsJson(PCWSTR(wide(&reply.to_string()).as_ptr()))
}

pub(super) unsafe fn deliver() {
    if !DOCUMENT.get() {
        return;
    }
    let Some(web) = CONTROLLER.with_borrow(|c| c.as_ref().and_then(|c| c.CoreWebView2().ok()))
    else {
        return;
    };
    if !local_document(&web).unwrap_or(false) {
        return;
    }
    let messages = SESSION
        .with_borrow(|s| {
            s.as_ref()
                .map(|s| std::mem::take(&mut *s.replies.messages.lock().unwrap()))
        })
        .unwrap_or_default();
    for value in messages {
        let _ = post(&web, &value);
    }
}

pub(super) unsafe fn attach(hwnd: HWND, web: &ICoreWebView2) -> windows::core::Result<()> {
    let mut token = 0;
    web.add_ContentLoading(
        &ContentLoadingEventHandler::create(Box::new(|web, _| {
            release();
            DOCUMENT.set(
                MANAGED.get()
                    && web
                        .as_ref()
                        .is_some_and(|w| local_document(w).unwrap_or(false)),
            );
            Ok(())
        })),
        &mut token,
    )?;
    web.add_WebMessageReceived(
        &WebMessageReceivedEventHandler::create(Box::new(move |web, args| {
            let (Some(web), Some(args)) = (web, args) else {
                return Ok(());
            };
            if !MANAGED.get() || !DOCUMENT.get() {
                return Ok(());
            }
            let mut raw = PWSTR::null();
            args.Source(&mut raw)?;
            if !take_pwstr(raw).starts_with(workspace::LOCAL_SITE) || !local_document(&web)? {
                return Ok(());
            }
            args.WebMessageAsJson(&mut raw)?;
            let Ok(value) = serde_json::from_str::<Value>(&take_pwstr(raw)) else {
                return Ok(());
            };
            let Some(id) = value["id"].as_u64() else {
                return Ok(());
            };
            if value["channel"].as_str() != Some(CHANNEL) {
                return Ok(());
            }
            let result = SESSION.with_borrow_mut(|slot| -> Result<(), String> {
                if slot.is_none() {
                    *slot = Some(Session::start(hwnd)?);
                }
                slot.as_ref()
                    .unwrap()
                    .requests
                    .send(Job::Request(value))
                    .map_err(|_| "设备连接已关闭".to_owned())
            });
            if let Err(error) = result {
                post(
                    &web,
                    &json!({"channel":CHANNEL,"id":id,"ok":false,"error":error}),
                )?;
            }
            Ok(())
        })),
        &mut token,
    )
}
