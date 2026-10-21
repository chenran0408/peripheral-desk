//! In-process JSON entry point shared by the four driver DLLs.
//! Native handles stay inside the driver; the host sees serializable values.
use crate::{devices::REGISTRY, protocol::Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
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
fn session(model: usize) -> Result<Box<dyn crate::adapters::DeviceSession>> {
    REGISTRY.get(model).ok_or("设备编号无效")?;
    let mut interfaces: Vec<_> = crate::hid::enumerate()?
        .into_iter()
        .filter(|i| crate::backend::matches(i, model))
        .collect();
    interfaces.sort_by_key(|i| {
        if i.vid == 0x1532 && i.pid == 0x00b6 {
            0
        } else {
            1
        }
    });
    let mut last = String::from("未检测到该设备的接口");
    for interface in &interfaces {
        match crate::adapters::open(model, interface) {
            Ok(session) => return Ok(session),
            Err(error) => last = error,
        }
    }
    Err(last)
}
fn dispatch(device_key: &str, input: &str) -> Result<Value> {
    if input.len() > 1048576 {
        return Err("驱动请求过大".into());
    }
    let request: Value =
        serde_json::from_str(input).map_err(|e| format!("驱动请求不是有效 JSON：{e}"))?;
    let model = REGISTRY
        .iter()
        .position(|spec| spec.key == device_key)
        .ok_or("该驱动不认识请求的设备型号")?;
    let operation = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or("缺少驱动操作类型")?;
    match operation {
        "read-state" => {
            let mut state = session(model)?.read()?;
            state.key = device_key.into();
            value(state)
        }
        "apply-setting" => {
            let action = field(&request, "action")?;
            let session = session(model)?;
            REGISTRY[model].validate(action, &session.read()?)?;
            session.apply(action)?;
            Ok(Value::Null)
        }
        "controls-read" => value(crate::controls::read(model)?),
        "controls-apply" => value(crate::controls::apply(model, &field(&request, "action")?)?),
        "fx-query" if device_key == "hecate-g2-pro" => {
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
        "fx-worker" if device_key == "hecate-g2-pro" => {
            value(crate::audio_fx::run_request(field(&request, "request")?)?)
        }
        "lighting-read" if device_key == "hecate-g2-pro" => value(crate::audio_lighting::read()?),
        "lighting-apply" if device_key == "hecate-g2-pro" => {
            value(crate::audio_lighting::apply(&field(&request, "config")?)?)
        }
        _ => Err("该驱动尚未提供此操作".into()),
    }
}
pub fn request(device_key: &str, input: &str) -> String {
    let reply = match dispatch(device_key, input) {
        Ok(value) => json!({"ok":true,"value":value,"error":""}),
        Err(error) => json!({"ok":false,"value":null,"error":error}),
    };
    reply.to_string()
}
