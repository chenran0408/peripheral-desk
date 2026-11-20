//! Model-scoped local templates. Keep the original JSON for re-export and
//! project only understood settings when the user applies a template.
mod a5;

use super::{apply_archive, call, checked_key, data, read_json, OperationReport};
use crate::{
    controls::{a5_binding, Binding, BindingAction, ControlState},
    hid::DeviceTarget,
    plugin_manager,
    protocol::Result,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const A5: &str = "mchose-a5-ultra";
const G75: &str = "mchose-g75-pro";
const FORMAT: &str = "peripheral-desk-device-template";

fn capability(key: &str, cap: &str) -> bool {
    crate::devices::REGISTRY
        .iter()
        .any(|s| s.key == key && s.supports(cap))
}
fn snapshot_provider(key: &str) -> Option<(&'static str, &'static str)> {
    if capability(key, "magnetic-settings") {
        Some((
            "mhub-magnetic-template-read",
            "mhub-magnetic-template-apply",
        ))
    } else if capability(key, "mchose-audio-templates") {
        Some(("mhub-audio-template-read", "mhub-audio-template-apply"))
    } else {
        None
    }
}
pub fn supports(key: &str) -> bool {
    matches!(key, G75 | A5)
        || capability(key, "mchose-mouse-templates")
        || snapshot_provider(key).is_some()
}

fn mouse_record<'a>(model: &str, document: &'a Value) -> Result<&'a Value> {
    let envelope = document
        .as_object()
        .filter(|o| o.len() == 1)
        .ok_or("请选择 M HUB 导出的单设备模板文件。")?;
    let (product, r) = envelope.iter().next().unwrap();
    let normalize = |s: &str| {
        let name = s.trim().to_ascii_lowercase();
        name.strip_prefix("mchose ")
            .or_else(|| name.strip_prefix("迈从 "))
            .unwrap_or(&name)
            .trim()
            .to_owned()
    };
    let actual = normalize(product);
    let expected = normalize(model);
    if actual != expected && !(expected == "a7 v3 magnetic" && actual == "a7 magnetic") {
        return Err("此模板属于其他型号，请选择当前鼠标的模板。".into());
    }
    if !r.is_object()
        || [
            "leftKeyType",
            "dpiSum",
            "usbRateIndex",
            "sleep",
            "sensor",
            "lodIndex",
            "adaptivePowerSwitch",
        ]
        .iter()
        .all(|f| r.get(f).is_none())
    {
        return Err("文件缺少 M HUB 鼠标的键位、DPI 或性能数据。".into());
    }
    Ok(r)
}
fn usb_id(v: &Value) -> Option<u16> {
    if let Some(n) = v.as_u64() {
        return n.try_into().ok();
    }
    let s = v.as_str()?;
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}
fn keyboard_profile(document: &Value, matches: impl Fn(u16, u16) -> bool) -> Result<Settings> {
    if document["dataScope"] != "KeyboardProfile"
        || document["version"] != 3
        || !document["data"].is_object()
    {
        return Err("请选择 M HUB 的 KeyboardProfile v3 文件。".into());
    }
    let identity = |obj: &Value| Some((usb_id(&obj["vendorId"])?, usb_id(&obj["productId"])?));
    let (vid, pid) = identity(document).ok_or("磁轴模板缺少 USB 型号标识。")?;
    if !matches(vid, pid)
        || document
            .get("identity")
            .is_some_and(|v| identity(v) != Some((vid, pid)))
    {
        return Err("磁轴模板的 USB 型号与所选键盘不同。".into());
    }
    Ok(Settings {
        name: name(document["data"]["name"].as_str())?,
        controls: Value::Null,
        light: None,
        detail: "M HUB 磁轴模板 · 应用到当前板载槽。灯效、宏与其他原版字段随文件保留。".into(),
    })
}
fn model_name(key: &str) -> Result<&'static str> {
    Ok(crate::devices::REGISTRY[checked_key(key)?].name)
}
pub fn hint(key: &str) -> &'static str {
    if key == A5 {
        "支持首代 A5 Ultra 的 M HUB 六键、休眠及传感器设置。"
    } else if key == G75 {
        "保存可读的普通层、Fn 层、Fn2 层和逐键灯光。"
    } else if capability(key, "mchose-mouse-templates") {
        "支持原版 M HUB 鼠标 JSON。模板应用到当前选中的设备和板载槽。"
    } else {
        "保存、导入和应用当前型号的本地设置。"
    }
}

#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub detail: String,
    document: Value,
    source: PathBuf,
}

struct Settings {
    name: String,
    controls: Value,
    light: Option<u8>,
    detail: String,
}

fn valid_light(mode: u8) -> bool {
    mode == 19 || crate::devices::LIGHTS.iter().any(|(v, _)| *v == mode)
}

fn name(text: Option<&str>) -> Result<String> {
    let text = text.unwrap_or("").trim();
    if text.is_empty() || text.chars().count() > 96 || text.chars().any(char::is_control) {
        return Err("模板名称需为 1 至 96 个字符。".into());
    }
    Ok(text.to_owned())
}

// Original M HUB mouseContLeftNew exports { [productName]: {name, allInfo, ...} }.
// Its first-generation 0x67 record has six buttonTypeN / buttonValueN pairs.
fn old_a5_fields(document: &Value) -> Result<Settings> {
    let envelope = document
        .as_object()
        .filter(|obj| obj.len() == 1)
        .ok_or("请选择原版 M HUB 导出的单设备 JSON 文件。")?;
    let (model, record) = envelope.iter().next().unwrap();
    if !["a5 ultra", "mchose a5 ultra"].contains(&model.trim().to_ascii_lowercase().as_str()) {
        return Err("此 M HUB 模板与首代 A5 Ultra 的型号不匹配。".into());
    }
    let info = record["allInfo"]
        .as_object()
        .ok_or("模板缺少首代 A5 的 allInfo 按键表。")?;
    let mut controls = ControlState::default();
    let mut unknown = 0;
    for index in 0..6 {
        let n = index + 1;
        let kind = info
            .get(&format!("buttonType{n}"))
            .and_then(Value::as_u64)
            .filter(|v| *v < 16);
        let code = info
            .get(&format!("buttonValue{n}"))
            .and_then(Value::as_str)
            .and_then(|v| v.strip_prefix("0x"))
            .filter(|v| v.len() == 6)
            .and_then(|v| u32::from_str_radix(v, 16).ok());
        let Some((kind, code)) = kind.zip(code) else {
            unknown += 1;
            continue;
        };
        let [_, a, b, c] = code.to_be_bytes();
        let action = a5_binding(index, [kind as u8, a, b, c]);
        if matches!(action, BindingAction::Unknown(_) | BindingAction::Macro(_))
            || matches!(action, BindingAction::Consumer(v) if ![0xcd,0xb5,0xb6,0xb7,0xe9,0xea,0xe2].contains(&v))
        {
            unknown += 1;
            continue;
        }
        controls.bindings.push(Binding {
            control: index as u16,
            layer: 0,
            label: format!("按键 {n}"),
            action,
            editable: true,
        });
    }
    let count = controls.bindings.len();
    if count == 0 && !info.contains_key("buttonType1") {
        return Err("没有识别到原版 A5 六键模板。".into());
    }
    let performance = a5::desired(document).len();
    Ok(Settings {
        name: name(record["name"].as_str())?,
        controls: json!(controls),
        light: None,
        detail: format!("M HUB · {count} 个可识别按键、{performance} 个性能参数。另有 {unknown} 个键位未转入；原版宏、DPI 档表和其他字段随文件保留。"),
    })
}

fn settings(key: &str, document: &Value) -> Result<Settings> {
    if !supports(key) {
        return Err("此型号尚无本地模板格式。".into());
    }
    if capability(key, "mchose-mouse-templates") {
        let r = mouse_record(model_name(key)?, document)?;
        let count = ["left", "right", "mid", "forward", "retreat", "dpi"]
            .iter()
            .filter(|k| {
                r.get(format!("{k}KeyType"))
                    .and_then(Value::as_str)
                    .is_some_and(|v| v != "ff")
            })
            .count();
        return Ok(Settings {
            name: name(r["name"].as_str())?,
            controls: Value::Null,
            light: None,
            detail: format!(
                "M HUB · {count} 个键位 · {} 个 DPI 档位。应用到设备当前板载槽。",
                r.get("dpiSum")
                    .and_then(Value::as_u64)
                    .map_or_else(|| "未指定".into(), |n| n.to_string())
            ),
        });
    }
    if snapshot_provider(key).is_some() {
        if capability(key, "magnetic-settings") && document["dataScope"] == "KeyboardProfile" {
            let spec = &crate::devices::REGISTRY[checked_key(key)?];
            return keyboard_profile(document, |vid, pid| {
                spec.extra_matchers
                    .iter()
                    .any(|m| m.vid == vid && m.pid == pid)
            });
        }
        if document["format"] != FORMAT
            || document["schema"] != 1
            || document["device"] != key
            || document["snapshot"]["model"] != key
            || document["snapshot"]["schema"] != 1
        {
            return Err("模板的设备型号或文件版本不匹配。".into());
        }
        return Ok(Settings {
            name: name(document["name"].as_str())?,
            controls: Value::Null,
            light: None,
            detail: format!("{} · 本地设备设置", model_name(key)?),
        });
    }
    if document["format"] != FORMAT {
        return if key == A5 {
            old_a5_fields(document)
        } else {
            Err("请选择为 G75 Pro 保存的外设盒模板。".into())
        };
    }
    if document["device"] != key || document["schema"] != 1 {
        return Err("模板的设备型号或文件版本不匹配。".into());
    }
    let controls = document
        .get("controls")
        .filter(|v| v.is_object())
        .ok_or("模板缺少按键配置。")?;
    // Preserve the complete source. The projection ignores individual actions
    // from newer plugins so an unknown action cannot hide the other keys.
    let mut state = ControlState::default();
    let mut unrecognized = 0;
    for value in controls["bindings"].as_array().into_iter().flatten() {
        match serde_json::from_value(value.clone()) {
            Ok(binding) => state.bindings.push(binding),
            Err(_) => unrecognized += 1,
        }
    }
    for value in controls["colors"].as_array().into_iter().flatten() {
        match serde_json::from_value(value.clone()) {
            Ok(color) => state.colors.push(color),
            Err(_) => unrecognized += 1,
        }
    }
    for (slot, value) in controls["macros"].as_object().into_iter().flatten() {
        match (slot.parse(), serde_json::from_value(value.clone())) {
            (Ok(slot), Ok(definition)) => {
                state.macros.insert(slot, definition);
            }
            _ => unrecognized += 1,
        }
    }
    let light = document
        .get("lightingMode")
        .and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .filter(|m| key == G75 && valid_light(*m));
    if state.bindings.is_empty() && state.colors.is_empty() && light.is_none() {
        return Err("没有识别到此设备的按键或灯光配置。".into());
    }
    let mut detail = if key == G75 {
        let layers: BTreeSet<_> = state.bindings.iter().map(|b| b.layer).collect();
        format!(
            "{} 层键位 · {} 个映射 · {} 个逐键颜色",
            layers.len(),
            state.bindings.len(),
            state.colors.len()
        )
    } else {
        format!(
            "{} 个按键映射 · {} 个已读回的板载宏 · {} 个性能参数",
            state.bindings.len(),
            state.macros.len(),
            a5::desired(document).len()
        )
    };
    if key == G75 && light.is_none() && !state.colors.is_empty() {
        detail.push_str("。未识别原灯效模式，逐键颜色仅随文件保存。");
        state.colors.clear();
    }
    if unrecognized != 0 {
        detail.push_str(&format!("。另有 {unrecognized} 项新版字段仅随文件保存。"));
    }
    Ok(Settings {
        name: name(document["name"].as_str())?,
        controls: json!(state),
        light,
        detail,
    })
}

fn directory(key: &str) -> Result<PathBuf> {
    checked_key(key)?;
    if !supports(key) {
        return Err("此型号尚无本地模板。".into());
    }
    Ok(data().join("devices").join(key).join("templates"))
}

fn item(key: &str, document: Value) -> Result<Template> {
    let parsed = settings(key, &document)?;
    Ok(Template {
        name: parsed.name,
        detail: parsed.detail,
        document,
        source: PathBuf::new(),
    })
}

pub fn list(key: &str) -> Result<Vec<Template>> {
    list_in(key, &directory(key)?)
}
fn list_in(key: &str, folder: &Path) -> Result<Vec<Template>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|p| p.to_str()) == Some("json") {
            if let Some(record) = read_json(&path)? {
                let mut template = item(key, record["document"].clone())?;
                template.name = name(record["name"].as_str())?;
                template.source = path;
                result.push(template);
            }
        }
    }
    result.sort_by_key(|t| t.name.to_lowercase());
    Ok(result)
}

fn store(folder: &Path, key: &str, document: Value, alias: &str) -> Result<Template> {
    let mut entry = item(key, document)?;
    if !alias.trim().is_empty() {
        entry.name = name(Some(alias))?;
    }
    let lower = entry.name.to_lowercase();
    if let Some(old) = list_in(key, folder)?
        .into_iter()
        .find(|i| i.name.to_lowercase() == lower)
    {
        if old.document == entry.document {
            return Ok(old);
        }
        return Err("此型号已有同名模板。请使用不同的模板名称。".into());
    }
    write_entry(folder, entry)
}

fn write_entry(folder: &Path, mut entry: Template) -> Result<Template> {
    let lower = entry.name.to_lowercase();
    // A stable file identity allows a rename to commit in one atomic replacement.
    // Older entries used the digest of their display name. New entries may reuse a renamed label.
    let hash = Sha256::digest(lower.as_bytes());
    let mut path = folder.join(format!("{hash:x}.json"));
    let mut suffix = 0;
    while path.exists() {
        suffix += 1;
        path = folder.join(format!("{hash:x}-{suffix}.json"));
    }
    let record = json!({"name":entry.name,"document":entry.document});
    let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    if bytes.len() > super::MAX_FILE {
        return Err("模板文件过大。".into());
    }
    crate::storage::atomic_write(&path, &bytes)?;
    entry.source = path;
    Ok(entry)
}
pub fn rename(key: &str, entry: &Template, title: &str) -> Result<Template> {
    rename_in(&directory(key)?, key, entry, title)
}
fn rename_in(dir: &Path, key: &str, entry: &Template, title: &str) -> Result<Template> {
    let title = name(Some(title))?;
    if list_in(key, dir)?
        .iter()
        .any(|i| i.source != entry.source && i.name.to_lowercase() == title.to_lowercase())
    {
        return Err("此型号已有同名模板。".into());
    }
    let mut old = read_json(&entry.source)?.ok_or("模板已移除，请重新打开模板列表。")?;
    old["name"] = json!(title);
    let doc = &mut old["document"];
    if doc["dataScope"] == "KeyboardProfile" {
        doc["data"]["name"] = json!(title)
    } else if doc["format"] == FORMAT {
        doc["name"] = json!(title);
    } else if let Some(record) = doc.as_object_mut().and_then(|r| r.values_mut().next()) {
        record["name"] = json!(title);
    }
    let bytes = serde_json::to_vec(&old).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(&entry.source, &bytes)?;
    Ok(Template {
        name: title,
        document: old["document"].clone(),
        ..entry.clone()
    })
}
pub fn delete(entry: &Template) -> Result<()> {
    std::fs::remove_file(&entry.source).map_err(|e| format!("删除模板失败：{e}"))
}

pub fn import(key: &str, path: &Path, alias: &str) -> Result<Template> {
    store(
        &directory(key)?,
        key,
        read_json(path)?.ok_or("找不到模板文件。")?,
        alias,
    )
}

pub fn supports_legacy_import(key: &str) -> bool {
    key == A5 || capability(key, "mchose-mouse-templates") || capability(key, "magnetic-settings")
}

pub fn import_legacy(key: &str, product: &str) -> Result<(Vec<Template>, String)> {
    let documents = crate::mhub::legacy_templates::read(key, product)?;
    import_legacy_in(&directory(key)?, key, documents)
}

fn import_legacy_in(
    folder: &Path,
    key: &str,
    documents: Vec<Value>,
) -> Result<(Vec<Template>, String)> {
    if documents.is_empty() {
        return Ok((Vec::new(), "此型号没有可导入的旧 M HUB 本地模板。".into()));
    }
    let existing = list_in(key, folder)?;
    let mut added = Vec::new();
    let mut duplicates = 0;
    let mut skipped = 0;
    let mut reason = None;
    for document in documents {
        if existing
            .iter()
            .chain(&added)
            .any(|t| t.document == document)
        {
            duplicates += 1;
            continue;
        }
        let mut entry = match item(key, document) {
            Ok(t) => t,
            Err(err) => {
                skipped += 1;
                reason.get_or_insert(err);
                continue;
            }
        };
        let base = entry.name.clone();
        for i in 2.. {
            if existing
                .iter()
                .chain(&added)
                .all(|t| t.name.to_lowercase() != entry.name.to_lowercase())
            {
                break;
            }
            let suffix = format!(" ({i})");
            let stem: String = base.chars().take(96 - suffix.len()).collect();
            entry.name = format!("{stem}{suffix}");
        }
        match write_entry(folder, entry) {
            Ok(t) => added.push(t),
            Err(err) => {
                let saved = added.len();
                return Ok((
                    added,
                    format!("已导入 {saved} 个模板，保存下一个模板时出错：{err}"),
                ));
            }
        }
    }
    let mut message = format!(
        "已导入 {} 个模板。已有 {duplicates} 个相同模板。",
        added.len()
    );
    if let Some(reason) = reason {
        message.push_str(&format!("\r\n有 {skipped} 个未导入。{reason}"));
    }
    Ok((added, message))
}

pub fn export(entry: &Template, path: &Path) -> Result<()> {
    crate::storage::atomic_write(
        path,
        &serde_json::to_vec(&entry.document).map_err(|e| e.to_string())?,
    )
}

pub fn capture(key: &str, target: &DeviceTarget, title: &str) -> Result<Template> {
    let title = name(Some(title))?;
    plugin_manager::with_device_target(key, Some(target), || {
        if capability(key, "mchose-mouse-templates") {
            let document = call(key, json!({"op":"template-read","name":title}))?;
            return store(&directory(key)?, key, document, "");
        }
        if let Some((op, _)) = snapshot_provider(key) {
            let snapshot = call(key, json!({"op":op}))?;
            let doc =
                json!({"format":FORMAT,"schema":1,"device":key,"name":title,"snapshot":snapshot});
            return store(&directory(key)?, key, doc, "");
        }
        let controls = call(key, json!({"op":"controls-read","extended_modes":true}))?;
        let mut document =
            json!({"format":FORMAT,"schema":1,"name":title,"device":key,"controls":controls});
        if key == G75 {
            if let Ok(state) = call(key, json!({"op":"read-state"})) {
                document["lightingMode"] = state["light"].clone();
            }
        } else if key == A5 {
            if let Ok(state) = call(key, json!({"op":"read-state"}))
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
            {
                document["performance"] = a5::capture(&state);
            }
        }
        store(&directory(key)?, key, document, "")
    })
}

pub fn apply(key: &str, target: &DeviceTarget, entry: &Template) -> Result<OperationReport> {
    let selected = settings(key, &entry.document)?;
    plugin_manager::with_device_target(key, Some(target), || {
        if let Some((_, op)) = snapshot_provider(key) {
            let document = if entry.document["dataScope"] == "KeyboardProfile" {
                &entry.document
            } else {
                &entry.document["snapshot"]
            };
            let value = call(key, json!({"op":op,"document":document}))?;
            let changed = value["changed"]
                .as_u64()
                .map(|v| v as usize)
                .or_else(|| value["applied"].as_bool().map(usize::from))
                .ok_or("插件未返回模板应用结果。")?;
            return Ok(OperationReport {
                changed,
                skipped: value["skipped"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                message: format!("已应用「{}」，{} 组设置有变化。", entry.name, changed),
            });
        }
        let protected = crate::resident_macros::list(key)?
            .library
            .recovery
            .into_iter()
            .filter(|r| r.target_instance.as_ref().is_none_or(|id| id == &target.id))
            .map(|r| (r.control, r.layer))
            .collect();
        if capability(key, "mchose-mouse-templates") {
            let r = mouse_record(model_name(key)?, &entry.document)?;
            let prepared = call(key, json!({"op":"template-prepare","record":r}))?;
            let profile = prepared["profile"]
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or("设备未回报板载配置号。")?;
            let changed = prepared["changed"].as_bool().ok_or("缺少模板设置确认。")?;
            let mut report = apply_archive(
                key,
                json!({"cached":{"controls":prepared["controls"]}}),
                None,
                &protected,
                Some((profile, changed.then_some(r))),
                &[],
            )?;
            if prepared["skipped_keys"].as_u64().unwrap_or(0) > 0 {
                report.skipped.push("未识别的按键操作随原文件保留。".into())
            }
            if prepared["adaptive_skipped"] == true {
                report.skipped.push("当前固件未启用自适应省电设置。".into())
            }
            return Ok(report);
        }
        let performance = if key == A5 {
            a5::desired(&entry.document)
        } else {
            vec![]
        };
        let mut report = apply_archive(
            key,
            json!({"cached":{"controls":selected.controls}}),
            selected.light,
            &protected,
            None,
            &performance,
        )?;
        if entry.document["format"] != FORMAT {
            report.skipped.push(selected.detail);
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{ControlAction, KeyColor};

    #[test]
    fn original_a5_model_envelope_round_trips_even_with_unmapped_fields() {
        let raw = json!({"A5 ultra":{"name":"板载配置","allInfo":{
            "buttonType1":0,"buttonValue1":"0x000000",
            "buttonType2":2,"buttonValue2":"0x020400",
            "buttonType3":3,"buttonValue3":"0xe90000",
            "buttonType4":4,"buttonValue4":"0x000000",
            "buttonType5":11,"buttonValue5":"0xabcdef",
            "buttonType6":0,"buttonValue6":"0x000000",
            "dpi0":800,"futureFlag":27
        },"marcoInfo":{"futureMacro":[99,31]},"futureData":{"retain":true}}});
        let wanted = old_a5_fields(&raw).unwrap();
        let bindings: Vec<Binding> =
            serde_json::from_value(wanted.controls["bindings"].clone()).unwrap();
        assert_eq!(
            bindings.iter().map(|b| b.control).collect::<Vec<_>>(),
            [0, 1, 2, 5]
        );
        assert_eq!(
            bindings[1].action,
            BindingAction::Keyboard {
                usage: 4,
                modifiers: 2
            }
        );
        assert_eq!(bindings[2].action, BindingAction::Consumer(0xe9));
        assert!(settings(G75, &raw).is_err());
        assert!(settings(A5, &json!({"A5 V2 Ultra":raw["A5 ultra"]})).is_err());
        let folder = std::env::temp_dir().join(format!(
            "pd-template-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let saved = folder.join("records");
        let entry = store(&saved, A5, raw.clone(), "我的设置").unwrap();
        store(&saved, A5, raw.clone(), "我的设置").unwrap();
        assert_eq!(list_in(A5, &saved).unwrap().len(), 1);
        let reread = list_in(A5, &saved).unwrap().pop().unwrap();
        assert_eq!(entry.name, reread.name);
        let archive = json!({"A5 Ultra":{"name":"我的设置","allInfo":{"buttonType1":0,"buttonValue1":"0x000000"},"extra":"原样保留"}});
        let migration = folder.join("migration");
        let (entries, _) =
            import_legacy_in(&migration, A5, vec![raw.clone(), archive.clone()]).unwrap();
        assert_eq!(entries.len(), 2);
        let mut conflict = archive.clone();
        conflict["A5 Ultra"]["extra"] = json!("不同内容");
        let (entries, _) =
            import_legacy_in(&migration, A5, vec![archive.clone(), conflict.clone()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "我的设置 (2)");
        assert_eq!(entries[0].document, conflict);
        assert!(import_legacy_in(&migration, A5, vec![archive, conflict])
            .unwrap()
            .0
            .is_empty());
        let path = folder.join("export.json");
        export(&reread, &path).unwrap();
        assert_eq!(read_json(&path).unwrap().unwrap(), raw);
        let renamed = rename_in(&saved, A5, &reread, "常用").unwrap();
        assert_eq!(renamed.document["A5 ultra"]["name"], "常用");
        assert_eq!(
            renamed.document["A5 ultra"]["futureData"],
            raw["A5 ultra"]["futureData"]
        );
        let second = store(&saved, A5, raw.clone(), "我的设置").unwrap();
        assert_ne!(second.source, renamed.source);
        assert!(rename_in(&saved, A5, &second, "常用").is_err());
        delete(&renamed).unwrap();
        assert_eq!(list_in(A5, &saved).unwrap().len(), 1);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn keyboard_profile_accepts_vendor_identity_and_rejects_other_layout_revisions() {
        let mut doc = json!({"version":3,"dataScope":"KeyboardProfile","vendorId":"0x3837","productId":12352,"identity":{"vendorId":14391,"productId":"0x3040"},"data":{"name":"办公"}});
        let check = |v, p| v == 0x3837 && p == 0x3040;
        assert_eq!(keyboard_profile(&doc, check).unwrap().name, "办公");
        doc["identity"]["productId"] = json!(12290);
        assert!(keyboard_profile(&doc, check).is_err());
    }
    #[test]
    fn new_mouse_envelope_checks_model_and_generations_before_hardware() {
        let doc = json!({"MCHOSE A7 Magnetic":{"name":"游戏","leftKeyType":"00","dpiSum":4,"other":[99]}});
        assert_eq!(
            mouse_record("迈从 A7 V3 Magnetic", &doc).unwrap()["other"],
            json!([99])
        );
        assert!(mouse_record("迈从 A7 V3", &doc).is_err());
        assert!(mouse_record("迈从 A5 V3 Ultra", &json!({"A5 Ultra":{"allInfo":{}}})).is_err());
    }
    #[test]
    fn g75_projects_known_entries_and_retains_future_entries_in_the_file() {
        let original = json!({"format":FORMAT,"schema":1,"device":G75,"name":"方案",
            "controls":{"colors":[{"control":2,"rgb":[1,2,3]}],"bindings":[
                {"control":2,"label":"A","layer":0,"action":"Disabled","editable":true},
                {"control":3,"label":"B","layer":1,"action":{"NewAction":true},"editable":true}
            ]},"lightingMode":19});
        let entry = item(G75, original.clone()).unwrap();
        let projection = settings(G75, &entry.document).unwrap();
        assert_eq!(projection.controls["bindings"].as_array().unwrap().len(), 1);
        assert_eq!(projection.light, Some(19));
        assert_eq!(entry.document, original);
        assert!(settings(A5, &entry.document).is_err());
        let mut no_mode = original;
        no_mode.as_object_mut().unwrap().remove("lightingMode");
        assert!(settings(G75, &no_mode).unwrap().controls["colors"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn g75_custom_mode_is_entered_and_restored_with_a_color_command() {
        let old = KeyColor {
            control: 1,
            rgb: [10, 20, 30],
        };
        let wanted = KeyColor {
            control: 1,
            rgb: [31, 32, 33],
        };
        let state = ControlState {
            colors: vec![old.clone()],
            ..Default::default()
        };
        let light = |mode| json!({"op":"apply-setting","action":{"Light":mode}});
        let colors = |c| ControlAction::Colors(vec![c]).request(None);
        let mut report = OperationReport::default();
        let plans = super::super::g75_light_plans(2, Some(19), &state, vec![], vec![], &mut report);
        assert_eq!(plans[0].request, colors(old.clone()));
        assert_eq!(plans[0].undo, [light(2)]);
        let plans = super::super::g75_light_plans(
            2,
            Some(3),
            &state,
            vec![wanted.clone()],
            vec![old.clone()],
            &mut report,
        );
        assert_eq!(plans[0].request, colors(wanted.clone()));
        assert_eq!(plans[1].request, light(3));
        assert_eq!(plans[1].undo, [colors(wanted)]);
        assert_eq!(plans[0].undo, [colors(old), light(2)]);
        assert!(
            super::super::g75_light_plans(2, Some(2), &state, vec![], vec![], &mut report)
                .is_empty()
        );
    }
}
