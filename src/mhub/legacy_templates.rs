//! On-demand import from the same WebView2 profile and origin as our old M HUB.
//! Only an inert document is served. No vendor page, device bridge or external
//! browser profile participates in migration.
use super::*;
use serde_json::{json, Value};
use std::path::Path;
use webview2_com::{
    wait_with_pump, CallDevToolsProtocolMethodCompletedHandler, WebResourceRequestedEventHandler,
};
use windows::{core::w, Win32::UI::Shell::SHCreateMemStream};

const DOCUMENT: &str = "https://mhub.peripheraldesk.test/_peripheral_desk/read-legacy-templates";
const READER: &str = include_str!("legacy_templates.js");

/// Called on demand by the local template library's background worker.
pub fn read(key: &str, product: &str) -> Result<Vec<Value>, String> {
    let profile = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("找不到当前用户的应用数据目录")?
        .join("PeripheralDesk")
        .join("MHub");
    if !profile.is_dir() {
        return Ok(Vec::new());
    }
    let spec = crate::devices::REGISTRY
        .iter()
        .find(|spec| spec.key == key)
        .ok_or("未安装此型号的插件")?;
    let keyboard = if spec.supports("magnetic-settings") {
        let device_type = spec.evidence["mhub_storage_type"]
            .as_u64()
            .ok_or("此型号的插件尚未提供旧磁轴模板库位置，请先更新插件。")?;
        let mut models = spec
            .extra_matchers
            .iter()
            .map(|m| (m.vid, m.pid))
            .collect::<Vec<_>>();
        models.sort_unstable();
        models.dedup();
        if models.is_empty() {
            return Err("旧磁轴模板库缺少设备型号信息。".into());
        }
        vec![json!({"type":device_type,"ids":models})]
    } else if key == "mchose-a5-ultra" || spec.supports("mchose-mouse-templates") {
        Vec::new()
    } else {
        return Err("此设备没有可导入的旧版模板库。".into());
    };
    let args =
        json!({"product":product, "firstGeneration":key == "mchose-a5-ultra", "keyboard":keyboard});
    let rows = thread::spawn(move || visit(&profile, &format!("({READER})({args})")))
        .join()
        .map_err(|_| "无法读取旧模板库")??;
    serde_json::from_value(rows).map_err(|e| format!("旧模板库格式无效：{e}"))
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

struct HiddenWindow(HWND);
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

struct ReaderView(ICoreWebView2Controller);
impl Drop for ReaderView {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Close();
        }
    }
}

fn visit(profile: &Path, expression: &str) -> Result<Value, String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let _apartment = Apartment;
        let window = CreateWindowExW(
            0,
            w!("STATIC").0,
            null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if window.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let window = HiddenWindow(window);
        inspect(&window, profile, expression)
            .map_err(|e| format!("无法读取旧版 M HUB 本地模板库：{e}"))
    }
}

unsafe fn inspect(
    window: &HiddenWindow,
    profile: &Path,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel();
    let path = workspace::local_wide(profile);
    CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        PCWSTR(path.as_ptr()),
        None,
        &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            let _ = tx.send(result.and_then(|_| env.ok_or_else(|| E_POINTER.into())));
            Ok(())
        })),
    )?;
    let env = wait_with_pump(rx)??;
    let (tx, rx) = mpsc::channel();
    env.CreateCoreWebView2Controller(
        ComWindow(window.0),
        &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, view| {
            let _ = tx.send(result.and_then(|_| view.ok_or_else(|| E_POINTER.into())));
            Ok(())
        })),
    )?;
    let controller = ReaderView(wait_with_pump(rx)??);
    controller.0.SetIsVisible(false)?;
    let web = controller.0.CoreWebView2()?;
    let settings = web.Settings()?;
    settings.SetAreHostObjectsAllowed(false)?;
    settings.SetIsWebMessageEnabled(false)?;
    // A previous workspace's service worker must not supply this document.
    devtools(
        &web,
        "Network.setBypassServiceWorker",
        &json!({"bypass":true}),
    )?;
    web.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
    let service = env.clone();
    let mut token = 0;
    web.add_WebResourceRequested(&WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
        let args = args.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        let mut uri = PWSTR::null();
        args.Request()?.Uri(&mut uri)?;
        let local = take_pwstr(uri) == DOCUMENT;
        let body = SHCreateMemStream(Some(b"<!doctype html><meta charset=utf-8>"));
        let response = service.CreateWebResourceResponse(body.as_ref(), if local {200} else {403},
            if local {w!("OK")} else {w!("Forbidden")},
            w!("Content-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'"))?;
        args.SetResponse(&response)
    })), &mut token)?;
    let (tx, rx) = mpsc::channel();
    web.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            let mut success = windows::core::BOOL::default();
            let outcome = args
                .ok_or_else(|| windows::core::Error::from(E_POINTER))
                .and_then(|args| args.IsSuccess(&mut success))
                .map(|_| success.as_bool());
            let _ = tx.send(outcome);
            Ok(())
        })),
        &mut token,
    )?;
    web.Navigate(PCWSTR(wide(DOCUMENT).as_ptr()))?;
    if !wait_with_pump(rx)?? {
        return Err("本地模板库页面未载入".into());
    }
    let result = devtools(
        &web,
        "Runtime.evaluate",
        &json!({"expression":expression, "returnByValue":true, "awaitPromise":true}),
    )?;
    if let Some(detail) = result.get("exceptionDetails") {
        return Err(detail["exception"]["description"]
            .as_str()
            .unwrap_or("旧模板无法解析")
            .to_owned()
            .into());
    }
    result["result"]
        .get("value")
        .cloned()
        .ok_or_else(|| "模板库未返回数据".into())
}

unsafe fn devtools(
    web: &ICoreWebView2,
    method: &str,
    params: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel();
    web.CallDevToolsProtocolMethod(
        PCWSTR(wide(method).as_ptr()),
        PCWSTR(wide(&params.to_string()).as_ptr()),
        &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            let _ = tx.send(result.map(|_| json));
            Ok(())
        })),
    )?;
    Ok(serde_json::from_str(&wait_with_pump(rx)??)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reopen a separate profile at the production origin. No dependency on
    // data from a live device, a browser session, or the user's own workspace.
    #[test]
    #[ignore = "requires installed Windows WebView2 runtime"]
    fn reads_two_mouse_generations_and_scoped_magnetic_profiles_offline() {
        let profile = std::env::temp_dir().join(format!(
            "PeripheralDesk-legacy-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let seed = r#"(async()=>{
          localStorage.mouse=JSON.stringify({
            'A5 ultra':{templateList:[{name:'old mouse',allInfo:{buttonType1:1}},null]},
            'MCHOSE A7 Magnetic':{templateList:[{name:'new local',dpiSum:5,extraValue:'retained'}]},
            'MCHOSE A7 V3':{templateList:[{name:'different model',dpiSum:4}]}
          });
          localStorage.deviceName='MCHOSE A7 V3';
          async function create(name,stores) {
            await new Promise((res,rej)=>{
              const open=indexedDB.open(name,1);
              open.onupgradeneeded=()=>{
                for(const [store,entries] of Object.entries(stores)) {
                  const os=open.result.createObjectStore(store);
                  for(const [key,value] of entries) os.put(value,key);
                }
              };
              open.onsuccess=()=>{open.result.close();res()};open.onerror=()=>rej(open.error);
            });
          }
          await create('mouse-custom',{'MCHOSE A7 Magnetic':[
            ['three',{name:'my saved profile',sleep:10,uid:'local-user',futureField:42}]
          ]});
          await create('mouse-cloud',{
            'MCHOSE A7 Magnetic':[['one',{name:'new cloud',leftKeyType:'01'}]],
            'MCHOSE A7 V3':[['two',{name:'other cloud',leftKeyType:'01'}]],
            'A5 ultra':[['two',{name:'old cloud',allInfo:{profileIndex:2}}]]
          });
          await create('cizhou@type-101',{local_data_v2:[
            ['profiles',[{name:'keys',triggerTravel:[91],extra:{identity:{vendorId:16868,productId:8449,productName:'Ace 60'}}},
            {name:'foreign',extra:{identity:{vendorId:16868,productId:1}}}]],
            ['__macros__',[{name:'macro'}]],['__profileNames__',['keys']]
          ]});
          return true;
        })()"#;
        visit(&profile, seed).unwrap();
        let inspect = |product: &str, first: bool, keyboard: Value| {
            let args = json!({"product":product,"firstGeneration":first,"keyboard":keyboard});
            visit(&profile, &format!("({READER})({args})")).unwrap()
        };
        let old = inspect("迈从 A5 Ultra", true, json!([]));
        assert_eq!(old.as_array().unwrap().len(), 2);
        assert_eq!(old[0]["A5 ultra"]["name"], "old mouse");
        let new = inspect("迈从 A7 V3 Magnetic", false, json!([]));
        assert_eq!(new.as_array().unwrap().len(), 3);
        assert_eq!(new[0]["MCHOSE A7 Magnetic"]["extraValue"], "retained");
        assert_eq!(new[1]["MCHOSE A7 Magnetic"]["name"], "my saved profile");
        assert!(new[1]["MCHOSE A7 Magnetic"].get("uid").is_none());
        assert_eq!(new[1]["MCHOSE A7 Magnetic"]["futureField"], 42);
        assert_eq!(new[2]["MCHOSE A7 Magnetic"]["name"], "new cloud");
        let keys = inspect("Ace 60", false, json!([{"type":101,"ids":[[16868,8449]]}]));
        assert_eq!(keys.as_array().unwrap().len(), 1);
        assert_eq!(keys[0]["dataScope"], "KeyboardProfile");
        assert_eq!(keys[0]["type"], "keyboard");
        assert_eq!(keys[0]["data"]["triggerTravel"][0], 91);
        assert_eq!(keys[0]["macros"][0]["name"], "macro");
        assert_eq!(inspect("迈从 A5 Ultra", true, json!([])), old);
        // WebView2 may still hold its cache open briefly on process shutdown.
        let _ = std::fs::remove_dir_all(&profile);
    }
}
