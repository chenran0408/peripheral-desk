//! Fixed-device JSON roots for independently linked driver DLLs.
//! Keep protocol callbacks out of shared dispatch: one DLL must not root other brands.
use crate::{
    adapters::{mouse::Channel, DeviceSession},
    controls::{ControlAction, ControlState},
    devices::{DeviceSpec, REGISTRY},
    hid::Interface,
    model::{Action, State},
    protocol::Result,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

const RAZER: &str = "razer-deathadder-v3-pro";
const MCHOSE: &str = "mchose-a5-ultra";
const G75: &str = "mchose-g75-pro";
const HECATE: &str = "hecate-g2-pro";
type Open = fn() -> Result<Box<dyn DeviceSession>>;
type ControlsRead = fn() -> Result<ControlState>;
type ControlsApply = fn(&ControlAction) -> Result<ControlState>;
type Extra = fn(&str, &Value) -> Result<Value>;
struct Endpoint {
    key: &'static str,
    open: Open,
    controls: Option<(ControlsRead, ControlsApply)>,
    extra: Option<Extra>,
}
const RAZER_ENDPOINT: Endpoint = Endpoint {
    key: RAZER,
    open: open_razer,
    controls: Some((read_razer_controls, apply_razer_controls)),
    extra: None,
};
const MCHOSE_ENDPOINT: Endpoint = Endpoint {
    key: MCHOSE,
    open: open_mchose,
    controls: Some((read_mchose_controls, apply_mchose_controls)),
    extra: None,
};
const G75_ENDPOINT: Endpoint = Endpoint {
    key: G75,
    open: open_g75,
    controls: Some((read_g75_controls, apply_g75_controls)),
    extra: None,
};
const HECATE_ENDPOINT: Endpoint = Endpoint {
    key: HECATE,
    open: open_hecate,
    controls: None,
    extra: Some(hecate_extra),
};

fn field<T: DeserializeOwned>(request: &Value, name: &str) -> Result<T> {
    serde_json::from_value(
        request
            .get(name)
            .cloned()
            .ok_or_else(|| format!("缺少驱动参数：{name}"))?,
    )
    .map_err(|e| format!("驱动参数 {name} 无效：{e}"))
}
fn value<T: Serialize>(data: T) -> Result<Value> {
    serde_json::to_value(data).map_err(|e| e.to_string())
}
fn parse(input: &str) -> Result<Value> {
    if input.len() > 1048576 {
        return Err("驱动请求过大".into());
    }
    serde_json::from_str(input).map_err(|e| format!("驱动请求不是有效 JSON：{e}"))
}
fn spec(key: &str) -> Result<&'static DeviceSpec> {
    REGISTRY
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| "该驱动不认识请求的设备型号".into())
}
// State sessions try every matched interface (wired Razer first); controls retain
// the original first-interface policy. Neither path uses the all-device opener.
fn connect<T>(
    key: &str,
    open: fn(&Interface, &'static DeviceSpec) -> Result<T>,
    state: bool,
) -> Result<T> {
    let spec = spec(key)?;
    let mut interfaces: Vec<_> = crate::hid::enumerate()?
        .into_iter()
        .filter(|i| spec.matches(i))
        .collect();
    if !state {
        return open(interfaces.first().ok_or("设备尚未连接")?, spec);
    }
    interfaces.sort_by_key(|i| {
        if i.vid == 0x1532 && i.pid == 0x00b6 {
            0
        } else {
            1
        }
    });
    let mut last = String::from("未检测到该设备的接口");
    for interface in &interfaces {
        match open(interface, spec) {
            Ok(session) => return Ok(session),
            Err(error) => last = error,
        }
    }
    Err(last)
}
struct FixedMouse {
    channel: Channel,
    read: fn(&Channel) -> Result<State>,
    apply: fn(&Channel, Action) -> Result<()>,
}
impl DeviceSession for FixedMouse {
    fn read(&self) -> Result<State> {
        (self.read)(&self.channel)
    }
    fn apply(&self, action: Action) -> Result<()> {
        (self.apply)(&self.channel, action)
    }
}
fn open_razer() -> Result<Box<dyn DeviceSession>> {
    Ok(Box::new(FixedMouse {
        channel: connect(RAZER, Channel::new_razer, true)?,
        read: Channel::read_razer,
        apply: Channel::apply_razer,
    }))
}
fn open_mchose() -> Result<Box<dyn DeviceSession>> {
    Ok(Box::new(FixedMouse {
        channel: connect(MCHOSE, Channel::new_mchose, true)?,
        read: Channel::read_mchose,
        apply: Channel::apply_mchose,
    }))
}
struct FixedKeyboard {
    keyboard: crate::keyboard::Keyboard,
    identity: String,
}
impl DeviceSession for FixedKeyboard {
    fn read(&self) -> Result<State> {
        self.keyboard.read(self.identity.clone())
    }
    fn apply(&self, action: Action) -> Result<()> {
        self.keyboard.apply(action)
    }
}
fn keyboard(i: &Interface, _: &'static DeviceSpec) -> Result<crate::keyboard::Keyboard> {
    let keyboard = crate::keyboard::Keyboard::open(&i.path)?;
    keyboard.verify_model()?;
    Ok(keyboard)
}
fn open_g75() -> Result<Box<dyn DeviceSession>> {
    connect(
        G75,
        |i, s| {
            Ok(Box::new(FixedKeyboard {
                keyboard: keyboard(i, s)?,
                identity: format!("{:04X}:{:04X}", i.vid, i.pid),
            }) as Box<dyn DeviceSession>)
        },
        true,
    )
}
fn open_hecate() -> Result<Box<dyn DeviceSession>> {
    connect(
        HECATE,
        |_, _| Ok(Box::new(crate::adapters::audio::AudioSession::open()?) as Box<dyn DeviceSession>),
        true,
    )
}
fn refreshed(read: ControlsRead) -> Result<ControlState> {
    read().map_err(|error| format!("设置已写入并核对，但刷新失败，请重新读取设备：{error}"))
}
fn read_razer_controls() -> Result<ControlState> {
    crate::adapters::razer_controls::read(&connect(RAZER, Channel::new_razer, false)?)
}
fn apply_razer_controls(action: &ControlAction) -> Result<ControlState> {
    crate::adapters::razer_controls::apply(&connect(RAZER, Channel::new_razer, false)?, action)?;
    refreshed(read_razer_controls)
}
fn read_mchose_controls() -> Result<ControlState> {
    crate::adapters::mchose_controls::read(&connect(MCHOSE, Channel::new_mchose, false)?)
}
fn apply_mchose_controls(action: &ControlAction) -> Result<ControlState> {
    crate::adapters::mchose_controls::apply(&connect(MCHOSE, Channel::new_mchose, false)?, action)?;
    refreshed(read_mchose_controls)
}
fn read_g75_controls() -> Result<ControlState> {
    crate::keyboard_controls::read(&connect(G75, keyboard, false)?)
}
fn apply_g75_controls(action: &ControlAction) -> Result<ControlState> {
    crate::keyboard_controls::apply(&connect(G75, keyboard, false)?, action)?;
    refreshed(read_g75_controls)
}
fn dispatch(endpoint: &Endpoint, device_key: &str, input: &str) -> Result<Value> {
    let request = parse(input)?;
    if device_key != endpoint.key {
        return Err("该驱动不认识请求的设备型号".into());
    }
    let operation = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or("缺少驱动操作类型")?;
    match operation {
        "read-state" => {
            let mut state = (endpoint.open)()?.read()?;
            state.key = device_key.into();
            value(state)
        }
        "apply-setting" => {
            let action = field(&request, "action")?;
            let session = (endpoint.open)()?;
            spec(endpoint.key)?.validate(action, &session.read()?)?;
            session.apply(action)?;
            Ok(Value::Null)
        }
        "controls-read" | "controls-apply" => {
            // Deserialize apply before connecting, as in the legacy dispatcher.
            let action = if operation == "controls-apply" {
                Some(field(&request, "action")?)
            } else {
                None
            };
            let Some((read, apply)) = endpoint.controls else {
                connect(endpoint.key, |_, _| Ok(()), false)?;
                return Err("该设备没有键位或逐键灯设置".into());
            };
            value(match action {
                Some(action) => apply(&action)?,
                None => read()?,
            })
        }
        _ => endpoint.extra.ok_or("该驱动尚未提供此操作")?(operation, &request),
    }
}
// This callback is reachable only from the headset endpoint.
fn hecate_extra(operation: &str, request: &Value) -> Result<Value> {
    match operation {
        "fx-query" => {
            let action = request
                .get("action")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| format!("音效动作无效：{e}"))?;
            let values: std::collections::BTreeMap<crate::model::Tuning, u16> = request
                .get("values")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| format!("音效参数无效：{e}"))?
                .unwrap_or_default();
            if action.is_some() && !values.is_empty() {
                return Err("音效请求不能同时包含单项和批量设置".into());
            }
            if values.is_empty() {
                value(crate::audio_fx::query(action)?)
            } else {
                value(crate::audio_fx::apply_profile(&values)?)
            }
        }
        "fx-worker" => value(crate::audio_fx::run_request(field(request, "request")?)?),
        "lighting-read" => value(crate::audio_lighting::read()?),
        "lighting-switch" => value(crate::audio_lighting::set_enabled(field(
            request, "enabled",
        )?)?),
        "lighting-apply" => value(crate::audio_lighting::apply(&field(request, "config")?)?),
        _ => Err("该驱动尚未提供此操作".into()),
    }
}
fn envelope(reply: Result<Value>) -> String {
    match reply {
        Ok(value) => json!({"ok":true,"value":value,"error":""}),
        Err(error) => json!({"ok":false,"value":null,"error":error}),
    }
    .to_string()
}
/// Fixed device root: passing another model is rejected before opening hardware.
pub fn request_razer(device_key: &str, input: &str) -> String {
    envelope(dispatch(&RAZER_ENDPOINT, device_key, input))
}
pub fn request_mchose(device_key: &str, input: &str) -> String {
    envelope(dispatch(&MCHOSE_ENDPOINT, device_key, input))
}
pub fn request_g75(device_key: &str, input: &str) -> String {
    envelope(dispatch(&G75_ENDPOINT, device_key, input))
}
pub fn request_hecate(device_key: &str, input: &str) -> String {
    envelope(dispatch(&HECATE_ENDPOINT, device_key, input))
}
/// SDK A compatibility. New DLL wrappers must use one of the fixed roots above.
pub fn request(device_key: &str, input: &str) -> String {
    match device_key {
        RAZER => request_razer(device_key, input),
        MCHOSE => request_mchose(device_key, input),
        G75 => request_g75(device_key, input),
        HECATE => request_hecate(device_key, input),
        _ => envelope(parse(input).and_then(|_| Err("该驱动不认识请求的设备型号".into()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_entry_contracts_without_hardware() {
        type Handler = fn(&str, &str) -> String;
        for (key, handler) in [
            (RAZER, request_razer as Handler),
            (MCHOSE, request_mchose),
            (G75, request_g75),
            (HECATE, request_hecate),
        ] {
            for input in [
                "{",
                "{}",
                r#"{"op":"not-supported"}"#,
                r#"{"op":"apply-setting"}"#,
                r#"{"op":"controls-apply"}"#,
            ] {
                let fixed: Value = serde_json::from_str(&handler(key, input)).unwrap();
                let legacy: Value = serde_json::from_str(&request(key, input)).unwrap();
                assert_eq!(fixed, legacy);
                assert_eq!(fixed["ok"], false);
                assert!(fixed["value"].is_null());
                assert!(!fixed["error"].as_str().unwrap().is_empty());
            }
            let rejected: Value =
                serde_json::from_str(&handler("other-device", r#"{"op":"read-state"}"#)).unwrap();
            assert_eq!(rejected["error"], "该驱动不认识请求的设备型号");
            if key != HECATE {
                let rejected: Value =
                    serde_json::from_str(&handler(key, r#"{"op":"fx-query"}"#)).unwrap();
                assert_eq!(rejected["error"], "该驱动尚未提供此操作");
                let rejected: Value = serde_json::from_str(&handler(
                    key,
                    r#"{"op":"lighting-switch","enabled":false}"#,
                ))
                .unwrap();
                assert_eq!(rejected["error"], "该驱动尚未提供此操作");
            }
        }
        for input in [
            r#"{"op":"lighting-switch"}"#,
            r#"{"op":"lighting-switch","enabled":"false"}"#,
        ] {
            let rejected: Value = serde_json::from_str(&request_hecate(HECATE, input)).unwrap();
            assert_eq!(rejected["ok"], false);
            assert!(rejected["error"].as_str().unwrap().contains("enabled"));
        }
        let invalid = request_hecate(
            HECATE,
            r#"{"op":"fx-query","action":["EqEnabled",1],"values":{"Eq30":20}}"#,
        );
        assert_eq!(
            serde_json::from_str::<Value>(&invalid).unwrap()["error"],
            "音效请求不能同时包含单项和批量设置"
        );
    }
    static CALLS: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());
    struct Fake;
    impl DeviceSession for Fake {
        fn read(&self) -> Result<State> {
            CALLS.lock().unwrap().push("read");
            Ok(State::default())
        }
        fn apply(&self, _: Action) -> Result<()> {
            CALLS.lock().unwrap().push("apply");
            Ok(())
        }
    }
    fn fake_open() -> Result<Box<dyn DeviceSession>> {
        CALLS.lock().unwrap().push("open");
        Ok(Box::new(Fake))
    }
    #[test]
    fn fixed_setting_validates_before_write_and_keeps_read_order() {
        let endpoint = Endpoint {
            key: RAZER,
            open: fake_open,
            controls: None,
            extra: None,
        };
        CALLS.lock().unwrap().clear();
        assert!(dispatch(
            &endpoint,
            RAZER,
            r#"{"op":"apply-setting","action":{"Dpi":101}}"#
        )
        .is_err());
        assert_eq!(*CALLS.lock().unwrap(), ["open", "read"]);
        CALLS.lock().unwrap().clear();
        assert_eq!(
            dispatch(
                &endpoint,
                RAZER,
                r#"{"op":"apply-setting","action":{"Dpi":1200}}"#
            )
            .unwrap(),
            Value::Null
        );
        assert_eq!(*CALLS.lock().unwrap(), ["open", "read", "apply"]);
    }
}
