//! Model-scoped local templates. Keep the original JSON for re-export and
//! project only understood settings when the user applies a template.
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

pub fn supports(key: &str) -> bool {
    matches!(key, G75 | A5)
}

#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub detail: String,
    document: Value,
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
    Ok(Settings {
        name: name(record["name"].as_str())?,
        controls: json!(controls),
        light: None,
        detail: format!("M HUB · {count} 个可识别按键。另有 {unknown} 个按键未转入；原版宏、性能和其他字段随文件保留。"),
    })
}

fn settings(key: &str, document: &Value) -> Result<Settings> {
    if !supports(key) {
        return Err("此型号尚无本地模板格式。".into());
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
            "{} 个按键映射 · {} 个已读回的板载宏",
            state.bindings.len(),
            state.macros.len()
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
    // A user-provided display name is never a path. Identical imports are idempotent.
    let hash = Sha256::digest(entry.name.to_lowercase().as_bytes());
    let path = folder.join(format!("{hash:x}.json"));
    match read_json(&path)? {
        Some(old) if old["document"] != entry.document => {
            return Err("此型号已有同名模板。请使用不同的模板名称。".into())
        }
        Some(_) => {}
        None => {
            let record = json!({"name":entry.name,"document":entry.document});
            let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
            if bytes.len() > super::MAX_FILE {
                return Err("模板文件过大。".into());
            }
            crate::storage::atomic_write(&path, &bytes)?;
        }
    }
    Ok(entry)
}

pub fn import(key: &str, path: &Path, alias: &str) -> Result<Template> {
    store(
        &directory(key)?,
        key,
        read_json(path)?.ok_or("找不到模板文件。")?,
        alias,
    )
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
        let controls = call(key, json!({"op":"controls-read","extended_modes":true}))?;
        let mut document =
            json!({"format":FORMAT,"schema":1,"name":title,"device":key,"controls":controls});
        if key == G75 {
            if let Ok(state) = call(key, json!({"op":"read-state"})) {
                document["lightingMode"] = state["light"].clone();
            }
        }
        store(&directory(key)?, key, document, "")
    })
}

pub fn apply(key: &str, target: &DeviceTarget, entry: &Template) -> Result<OperationReport> {
    let selected = settings(key, &entry.document)?;
    plugin_manager::with_device_target(key, Some(target), || {
        let protected = crate::resident_macros::list(key)?
            .library
            .recovery
            .into_iter()
            .filter(|r| r.target_instance.as_ref().is_none_or(|id| id == &target.id))
            .map(|r| (r.control, r.layer))
            .collect();
        let mut report = apply_archive(
            key,
            json!({"cached":{"controls":selected.controls}}),
            selected.light,
            &protected,
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
        let path = folder.join("export.json");
        export(&reread, &path).unwrap();
        assert_eq!(read_json(&path).unwrap().unwrap(), raw);
        std::fs::remove_dir_all(folder).unwrap();
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
