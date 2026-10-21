//! Portable per-device configuration archives and privacy-safe diagnostics.
//! Import restores local records; hardware application is a separate transaction.
use crate::{
    audio_fx,
    audio_lighting::{LightingConfig, LightingState},
    controls::{BindingAction, ControlAction, ControlState},
    devices::REGISTRY,
    model::{Action, State, Tuning},
    plugin_manager,
    protocol::Result,
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const MAX_FILE: usize = 8 * 1024 * 1024;
const CACHE_NAMES: [&str; 5] = [
    "state",
    "controls",
    "lighting",
    "effects",
    "resident-macros",
];
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OperationReport {
    pub changed: usize,
    pub skipped: Vec<String>,
    pub message: String,
}
fn data() -> PathBuf {
    crate::backend::data_path().parent().unwrap().to_owned()
}
fn checked_key(key: &str) -> Result<usize> {
    if key.is_empty()
        || key.len() > 80
        || !key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("设备标识无效".into());
    }
    REGISTRY
        .iter()
        .position(|spec| spec.key == key)
        .ok_or_else(|| "配置中的设备尚未加入当前设备目录".into())
}
fn device_dir(key: &str) -> PathBuf {
    data().join("devices").join(key)
}
fn local_files(key: &str) -> Vec<(&'static str, &'static str)> {
    match key {
        "mchose-a5-ultra" => vec![("a5-macros", "a5-macros.json")],
        "hecate-g2-pro" => vec![
            ("eq-presets", "eq-presets.json"),
            ("g2-lighting", "g2-pro-lighting.json"),
        ],
        _ => vec![],
    }
}
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("配置文件过大".into());
    }
    Ok(Some(bytes))
}
fn read_json(path: &Path) -> Result<Option<Value>> {
    read_bytes(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|e| format!("配置 JSON 格式无效：{e}")))
        .transpose()
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    storage::atomic_write(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}
fn merge(target: &mut Value, incoming: Value) {
    match (target, incoming) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, value) in new {
                if let Some(current) = old.get_mut(&key) {
                    merge(current, value)
                } else {
                    old.insert(key, value);
                }
            }
        }
        (target, value) => *target = value,
    }
}
fn portable(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter_map(|(key, value)| {
                    let low = key.to_ascii_lowercase();
                    if [
                        "path",
                        "directory",
                        "serial",
                        "username",
                        "sdk",
                        "identity",
                        "error",
                        "notice",
                    ]
                    .iter()
                    .any(|part| low.contains(part))
                    {
                        None
                    } else {
                        Some((key, portable(value)))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(portable).collect()),
        other => other,
    }
}
fn collect(key: &str) -> Result<Value> {
    checked_key(key)?;
    let mut cached = serde_json::Map::new();
    for name in CACHE_NAMES {
        if let Some(value) = read_json(&device_dir(key).join(format!("{name}.json")))? {
            cached.insert(
                name.into(),
                if name == "resident-macros" {
                    crate::resident_macros::export_value(value)
                } else {
                    value
                },
            );
        }
    }
    let mut local = serde_json::Map::new();
    for (alias, file) in local_files(key) {
        if let Some(value) = read_json(&data().join(file))? {
            local.insert(alias.into(), value);
        }
    }
    let mut archive = read_json(&device_dir(key).join("imported-configuration.json"))?
        .unwrap_or_else(|| json!({}));
    merge(
        &mut archive,
        json!({"schema":1,"device":key,"appVersion":env!("CARGO_PKG_VERSION"),"cached":cached,"local":local}),
    );
    if let Some(resident) = archive["cached"].get_mut("resident-macros") {
        *resident = crate::resident_macros::export_value(resident.take());
    }
    Ok(archive)
}
pub fn export_configuration(key: &str, path: &Path) -> Result<OperationReport> {
    let archive = portable(collect(key)?);
    write_json(path, &archive)?;
    Ok(OperationReport {
        message: "配置已导出。包含最近缓存与本地预设，不含驱动、SDK 路径或接口串号。".into(),
        ..OperationReport::default()
    })
}
fn normalize(mut value: Value, key: &str) -> Result<Value> {
    checked_key(key)?;
    if !value.is_object() {
        return Err("配置必须为 JSON 对象".into());
    }
    let found = value
        .get("device")
        .and_then(Value::as_str)
        .or_else(|| value.get("device_key").and_then(Value::as_str))
        .or_else(|| value.get("key").and_then(Value::as_str))
        .ok_or("旧配置缺少稳定设备标识，不能推断目标设备")?;
    if found != key {
        return Err("配置所属设备与所选设备不同，已拒绝导入".into());
    }
    let schema = match value.get("schema") {
        Some(value) => value.as_u64().ok_or("配置版本必须是非负整数")?,
        None => 0,
    };
    if schema > 1 {
        return Err("该配置版本尚不支持".into());
    }
    let mut cached = value.get("cached").cloned().unwrap_or_else(|| json!({}));
    if !cached.is_object() {
        return Err("配置 cached 字段应为对象".into());
    }
    for name in CACHE_NAMES {
        if let Some(old) = value.get(name) {
            cached[name] = old.clone();
        }
    }
    if let Some(values) = value.get("values") {
        if key == "hecate-g2-pro" {
            cached["effects"] = json!({"values":values});
        }
    }
    if value.get("bindings").is_some() {
        cached["controls"] = value.clone();
    }
    if value.get("tuning").is_some() && value.get("cached").is_none() {
        cached["state"] = value.clone();
    }
    let map = value.as_object_mut().unwrap();
    map.insert("schema".into(), json!(1));
    map.insert("device".into(), json!(key));
    map.insert("cached".into(), cached);
    if !map.contains_key("local") {
        map.insert("local".into(), json!({}));
    }
    if !map["local"].is_object() {
        return Err("配置 local 字段应为对象".into());
    }
    Ok(value)
}
fn target_files(key: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = CACHE_NAMES
        .iter()
        .map(|name| device_dir(key).join(format!("{name}.json")))
        .collect();
    paths.extend(
        local_files(key)
            .into_iter()
            .map(|(_, file)| data().join(file)),
    );
    paths.push(device_dir(key).join("imported-configuration.json"));
    paths
}
fn backup(key: &str) -> Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let directory = data()
        .join("configuration-backups")
        .join(key)
        .join(stamp.to_string());
    let mut records = Vec::new();
    for (index, path) in target_files(key).into_iter().enumerate() {
        if let Some(bytes) = read_bytes(&path)? {
            let name = format!("record-{index}.json");
            storage::atomic_write(&directory.join(&name), &bytes)?;
            records.push(json!({"record":name,"target":path.strip_prefix(data()).map_err(|e|e.to_string())?.to_string_lossy()}));
        }
    }
    write_json(
        &directory.join("manifest.json"),
        &json!({"schema":1,"device":key,"records":records}),
    )
}
fn valid_a5_macro(definition: &crate::controls::MacroDefinition) -> bool {
    use crate::controls::{MacroEvent, MacroMode};
    if definition.mode != MacroMode::Once
        || definition.name.trim().is_empty()
        || definition.name.len() > 20
        || definition.name.contains('\0')
        || definition.events.is_empty()
        || definition.events.len() > 13
    {
        return false;
    }
    let mut held = BTreeSet::new();
    for event in &definition.events {
        match *event {
            MacroEvent::Key { usage, pressed } if (4..=231).contains(&usage) => {
                if pressed {
                    if !held.insert(usage) {
                        return false;
                    }
                } else if !held.remove(&usage) {
                    return false;
                }
            }
            MacroEvent::Delay(ms) if ms > 0 => {}
            _ => return false,
        }
    }
    held.is_empty()
}
fn valid_local(alias: &str, value: &Value) -> bool {
    match alias {
        "a5-macros" => {
            serde_json::from_value::<BTreeMap<u16, crate::controls::MacroDefinition>>(value.clone())
                .is_ok_and(|macros| {
                    macros
                        .iter()
                        .all(|(control, definition)| *control < 6 && valid_a5_macro(definition))
                })
        }
        "g2-lighting" => serde_json::from_value::<LightingConfig>(value.clone())
            .is_ok_and(|config| (1..=10).contains(&config.speed)),
        "eq-presets" => serde_json::from_value::<crate::profiles::EqLibrary>(value.clone())
            .is_ok_and(|library| {
                let ids: BTreeSet<_> = library.presets.iter().map(|preset| preset.id).collect();
                let names: BTreeSet<_> = library
                    .presets
                    .iter()
                    .map(|preset| preset.name.to_lowercase())
                    .collect();
                ids.len() == library.presets.len()
                    && names.len() == library.presets.len()
                    && !ids.contains(&0)
                    && library.last_applied.is_none_or(|id| ids.contains(&id))
                    && library.presets.iter().all(|preset| {
                        !preset.name.trim().is_empty()
                            && preset.name.chars().count() <= 32
                            && !preset.name.chars().any(char::is_control)
                    })
                    && value.get("schema") == Some(&json!(1))
                    && value.get("device") == Some(&json!("hecate-g2-pro"))
                    && library.presets.len() <= 64
                    && library.presets.iter().all(|p| {
                        p.values.len() == 11
                            && p.values.keys().all(|key| {
                                *key == Tuning::EqEnabled
                                    || audio_fx::EQ_BANDS.iter().any(|(k, _)| key == k)
                            })
                            && crate::profiles::validate_audio_values(&p.values).is_ok()
                    })
            }),
        _ => false,
    }
}
pub fn import_configuration(key: &str, path: &Path) -> Result<OperationReport> {
    let incoming = normalize(read_json(path)?.ok_or("未找到配置文件")?, key)?;
    let mut report = OperationReport::default();
    let mut writes = Vec::new();
    for name in CACHE_NAMES {
        if let Some(value) = incoming["cached"]
            .get(name)
            .filter(|value| value.is_object())
        {
            let target = device_dir(key).join(format!("{name}.json"));
            if name == "resident-macros" {
                match crate::resident_macros::import_value(key, value.clone(), read_json(&target)?)
                {
                    Ok(value) => writes.push((target, value)),
                    Err(_) => report.skipped.push(
                        "软件宏定义无效或不适用于该设备，已保留本机恢复记录与导入归档。".into(),
                    ),
                }
            } else {
                writes.push((target, value.clone()));
            }
        }
    }
    for (alias, file) in local_files(key) {
        if let Some(value) = incoming["local"].get(alias) {
            if valid_local(alias, value) {
                writes.push((data().join(file), value.clone()));
            } else {
                report.skipped.push(format!("{alias} 含当前版本不能安全恢复的字段，完整内容已存入归档，现有本地记录未覆盖。"));
            }
        }
    }
    writes.push((
        device_dir(key).join("imported-configuration.json"),
        incoming,
    ));
    let mut prepared = Vec::new();
    for (path, incoming) in writes {
        let old = read_bytes(&path)?;
        let mut next = old
            .as_ref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_else(|| json!({}));
        merge(&mut next, incoming);
        if let Some((alias, _)) = local_files(key)
            .into_iter()
            .find(|(_, file)| data().join(file) == path)
        {
            if !valid_local(alias, &next) {
                report.skipped.push(format!(
                    "{alias} 原记录包含不能安全合并的字段，已保留原文件与导入归档。"
                ));
                continue;
            }
        }
        prepared.push((path, old, next));
    }
    backup(key)?;
    report.changed = commit_records(prepared, |path, bytes| {
        if let Some(bytes) = bytes {
            storage::atomic_write(path, bytes)
        } else {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    })?;
    report.message =
        "已恢复本地配置并保留未知字段；尚未写入硬件。原记录已备份，可单独点击“应用到设备”。".into();
    report
        .skipped
        .push("电量和性能快照只恢复本地；未知字段仅保存，不执行。".into());
    report
        .skipped
        .push("软件宏只导入本地定义，不自动启用；本机原绑定恢复记录不被导入内容覆盖。".into());
    Ok(report)
}
fn commit_records<F>(
    prepared: Vec<(PathBuf, Option<Vec<u8>>, Value)>,
    mut write: F,
) -> Result<usize>
where
    F: FnMut(&Path, Option<&[u8]>) -> Result<()>,
{
    let mut prior = Vec::new();
    for (path, old, next) in prepared {
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        prior.push((path.clone(), old));
        if let Err(error) = write(&path, Some(&bytes)) {
            let mut failed = false;
            for (path, old) in prior.into_iter().rev() {
                failed |= write(&path, old.as_deref()).is_err();
            }
            return Err(format!(
                "本地导入失败：{error}；{}",
                if failed {
                    "部分本地文件恢复失败，请使用备份"
                } else {
                    "已恢复原本地配置"
                }
            ));
        }
    }
    Ok(prior.len())
}
struct Plan {
    request: Value,
    undo: Vec<Value>,
}
fn binding_request(control: u16, layer: u8, action: BindingAction) -> Value {
    json!({"op":"controls-apply","action":ControlAction::Bind{control,layer,action}})
}
fn macro_request(control: u16, definition: crate::controls::MacroDefinition) -> Value {
    json!({"op":"controls-apply","action":ControlAction::Macro{control,definition}})
}
fn control_state(value: Value) -> Result<ControlState> {
    let mut complete = serde_json::to_value(ControlState::default()).map_err(|e| e.to_string())?;
    merge(&mut complete, value);
    serde_json::from_value(complete).map_err(|e| format!("键位配置格式不支持：{e}"))
}
fn call(key: &str, request: Value) -> Result<Value> {
    plugin_manager::invoke(key, request)
}
pub fn apply_configuration(key: &str) -> Result<OperationReport> {
    crate::resident_macros::disable_device(key)?;
    checked_key(key)?;
    let source = match read_json(&device_dir(key).join("imported-configuration.json"))? {
        Some(source) => source,
        None => collect(key)?,
    };
    let archive = normalize(source, key)?;
    let mut report = OperationReport::default();
    let mut plans = Vec::new();
    backup(key)?;
    if archive["cached"].get("resident-macros").is_some() {
        report
            .skipped
            .push("软件宏需在独立窗口手动启用，配置应用不会自动改写宏触发键。".into());
    }
    // Baselines for every requested group are read before the first write.
    if archive["cached"].get("state").is_some() {
        report
            .skipped
            .push("性能与电量快照不会自动写入设备；请在详情页调整性能设置。".into());
    }
    if let Some(raw) = archive["cached"].get("effects") {
        if key != "hecate-g2-pro" {
            report
                .skipped
                .push("该设备未提供已确认的耳机音效接口。".into());
        } else if let Some(wanted) = raw.get("values").and_then(Value::as_object) {
            let baseline = call(key, json!({"op":"fx-query"}))?;
            let baseline: BTreeMap<Tuning, u16> = serde_json::from_value(
                baseline.get("values").cloned().unwrap_or_else(|| json!({})),
            )
            .map_err(|e| e.to_string())?;
            let mut desired = BTreeMap::new();
            let mut old = BTreeMap::new();
            for (name, value) in wanted {
                let Ok(tuning) = serde_json::from_value::<Tuning>(json!(name)) else {
                    report.skipped.push("未知音效参数已保留并跳过。".into());
                    continue;
                };
                let Some(spec) = audio_fx::spec(tuning) else {
                    report.skipped.push("非音效参数已跳过。".into());
                    continue;
                };
                let Some(number) = value.as_u64().filter(|n| *n <= u64::from(spec.max)) else {
                    report.skipped.push("越界音效参数已跳过。".into());
                    continue;
                };
                let Some(original) = baseline.get(&tuning) else {
                    report
                        .skipped
                        .push("当前驱动没有提供某项音效，已跳过。".into());
                    continue;
                };
                if *original != number as u16 {
                    desired.insert(tuning, number as u16);
                    old.insert(tuning, *original);
                }
            }
            if !desired.is_empty() {
                plans.push(Plan {
                    request: json!({"op":"fx-query","values":desired}),
                    undo: vec![json!({"op":"fx-query","values":old})],
                });
            }
        } else {
            report
                .skipped
                .push("音效配置没有可识别的参数表，已保留原值。".into());
        }
    }
    if let Some(raw) = archive["cached"].get("controls") {
        match control_state(raw.clone()) {
            Err(_) => report
                .skipped
                .push("键位配置含有当前版本无法识别的内容，仅保存在本地。".into()),
            Ok(wanted) => {
                let baseline = control_state(call(key, json!({"op":"controls-read"}))?)?;
                for binding in wanted.bindings {
                    let Some(original) = baseline
                        .bindings
                        .iter()
                        .find(|b| b.control == binding.control && b.layer == binding.layer)
                    else {
                        report.skipped.push("不存在的按键/层已跳过。".into());
                        continue;
                    };
                    if !original.editable || matches!(binding.action, BindingAction::Unknown(_)) {
                        report.skipped.push("保留按键或未知映射已跳过。".into());
                        continue;
                    }
                    if matches!(
                        binding.action,
                        BindingAction::Keyboard {
                            usage: 104..=115,
                            ..
                        }
                    ) {
                        report
                            .skipped
                            .push("F13–F24 为软件宏保留触发键，不从配置快照自动下发。".into());
                        continue;
                    }
                    let undo = if matches!(original.action, BindingAction::Macro(_)) {
                        if let Some(definition) = baseline.macros.get(&binding.control) {
                            macro_request(binding.control, definition.clone())
                        } else {
                            report
                                .skipped
                                .push("设备当前宏缺少可验证的本工具备份，该按键未改动。".into());
                            continue;
                        }
                    } else if matches!(original.action, BindingAction::Unknown(_)) {
                        report
                            .skipped
                            .push("当前未知映射无法完整恢复，该按键未改动。".into());
                        continue;
                    } else {
                        binding_request(original.control, original.layer, original.action.clone())
                    };
                    if matches!(binding.action, BindingAction::Macro(_)) {
                        if !baseline.macro_supported {
                            report
                                .skipped
                                .push("该设备尚未开放安全的设备宏写入，已跳过。".into());
                            continue;
                        }
                        let Some(definition) = wanted.macros.get(&binding.control) else {
                            report
                                .skipped
                                .push("宏只有槽位编号，没有本工具的完整事件，已跳过。".into());
                            continue;
                        };
                        let identical = baseline.macros.get(&binding.control).is_some_and(|old| {
                            serde_json::to_value(old).ok() == serde_json::to_value(definition).ok()
                        });
                        if !identical {
                            plans.push(Plan {
                                request: macro_request(binding.control, definition.clone()),
                                undo: vec![undo],
                            });
                        }
                    } else if binding.action != original.action {
                        plans.push(Plan {
                            request: binding_request(
                                binding.control,
                                binding.layer,
                                binding.action,
                            ),
                            undo: vec![undo],
                        });
                    }
                }
                let mut desired = Vec::new();
                let mut old = Vec::new();
                let mut seen = BTreeSet::new();
                for color in wanted.colors {
                    if !seen.insert(color.control) {
                        continue;
                    }
                    if let Some(original) =
                        baseline.colors.iter().find(|c| c.control == color.control)
                    {
                        if original.rgb != color.rgb {
                            desired.push(color);
                            old.push(original.clone());
                        }
                    } else {
                        report.skipped.push("未知逐键灯位置已跳过。".into());
                    }
                }
                if !desired.is_empty() {
                    let state: State =
                        serde_json::from_value(call(key, json!({"op":"read-state"}))?)
                            .map_err(|e| e.to_string())?;
                    if key == "mchose-g75-pro"
                        && state.light.is_some_and(|mode| {
                            mode == 19 || crate::devices::LIGHTS.iter().any(|(id, _)| *id == mode)
                        })
                    {
                        let mut undo = vec![
                            json!({"op":"controls-apply","action":ControlAction::Colors(old)}),
                        ];
                        if state.light != Some(19) {
                            undo.push(json!({"op":"apply-setting","action":Action::Light(state.light.unwrap())}));
                        }
                        plans.push(Plan{request:json!({"op":"controls-apply","action":ControlAction::Colors(desired)}),undo});
                    } else {
                        report
                            .skipped
                            .push("无法确认并恢复原灯效模式，逐键颜色未写入。".into());
                    }
                }
            }
        }
    }
    if let Some(raw) = archive["cached"]
        .get("lighting")
        .or_else(|| archive["local"].get("g2-lighting"))
    {
        if key == "hecate-g2-pro" {
            let config = serde_json::from_value::<LightingConfig>(
                raw.get("saved")
                    .filter(|v| !v.is_null())
                    .unwrap_or(raw)
                    .clone(),
            );
            if let Ok(wanted) = config {
                let baseline: LightingState =
                    serde_json::from_value(call(key, json!({"op":"lighting-read"}))?)
                        .map_err(|e| e.to_string())?;
                if let Some(mut original) = baseline.saved {
                    original.enabled = baseline.enabled;
                    if wanted != original {
                        plans.push(Plan {
                            request: json!({"op":"lighting-apply","config":wanted}),
                            undo: vec![json!({"op":"lighting-apply","config":original})],
                        });
                    }
                    report.skipped.push(
                        "G2 灯光仅开关可从设备读回；颜色/模式按本工具保存记录应用与恢复。".into(),
                    );
                } else {
                    report
                        .skipped
                        .push("没有可恢复的原 G2 灯效记录，已跳过灯光写入。".into());
                }
            } else {
                report
                    .skipped
                    .push("灯光配置没有完整颜色/模式，已保留原灯光。".into());
            }
        }
    }
    // Establish an imported primary click before moving the old one, so a
    // valid full mapping does not fail the adapter's last-left-click guard.
    plans.sort_by_key(|plan| {
        let action = plan
            .request
            .get("action")
            .cloned()
            .and_then(|value| serde_json::from_value::<ControlAction>(value).ok());
        if matches!(
            action,
            Some(ControlAction::Bind {
                action: BindingAction::Mouse(1),
                ..
            })
        ) {
            0
        } else {
            1
        }
    });
    let mut attempted = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        attempted.push(index);
        if let Err(error) = call(key, plan.request.clone()) {
            let mut failed = 0;
            for index in attempted.into_iter().rev() {
                for undo in &plans[index].undo {
                    if call(key, undo.clone()).is_err() {
                        failed += 1;
                    }
                }
            }
            return Err(format!(
                "应用中断：{error}；{}",
                if failed == 0 {
                    "已逐项恢复原设置"
                } else {
                    "部分原设置恢复失败，请查看设备并重新读取"
                }
            ));
        }
        report.changed += 1;
    }
    report.skipped.sort();
    report.skipped.dedup();
    report.message = if report.changed == 0 {
        "没有需要写入的安全差异项；设备保持原值。".into()
    } else {
        format!("已应用 {} 组差异设置，并由设备驱动核对。", report.changed)
    };
    Ok(report)
}
fn error_category(error: &str) -> &'static str {
    let low = error.to_ascii_lowercase();
    if error.is_empty() {
        "none"
    } else if low.contains("timeout") || error.contains("超时") {
        "timeout"
    } else if error.contains("尚未安装") {
        "driver-not-installed"
    } else if low.contains("hash") || error.contains("校验") {
        "verification-failed"
    } else if error.contains("唤醒") || error.contains("未连接") || error.contains("未检测")
    {
        "device-unavailable"
    } else if error.contains("音效") || error.contains("组件") {
        "audio-component-unavailable"
    } else {
        "operation-failed"
    }
}
fn anonymous_interface(interface: &crate::hid::Interface) -> Value {
    json!({"vid":interface.vid,"pid":interface.pid,"usage_page":interface.usage_page,"usage":interface.usage,"feature_length":interface.feature_len,"input_length":interface.input_len,"output_length":interface.output_len})
}
pub fn export_diagnostics(key: Option<&str>, path: &Path) -> Result<OperationReport> {
    let models: Vec<usize> = if let Some(key) = key {
        vec![checked_key(key)?]
    } else {
        (0..REGISTRY.len()).collect()
    };
    let (interfaces, enumeration) = match crate::hid::enumerate() {
        Ok(all) => (all, "ok"),
        Err(_) => (vec![], "unavailable"),
    };
    let mut devices = Vec::new();
    for model in models {
        let spec = &REGISTRY[model];
        let plugin = match plugin_manager::installed(spec.key) {
            Ok(Some(driver)) => {
                json!({"id":driver.id,"version":driver.version,"abi":driver.manifest.abi})
            }
            Ok(None) => json!({"id":spec.key,"installed":false}),
            Err(_) => json!({"id":spec.key,"status":"metadata-unavailable"}),
        };
        let ports: Vec<_> = interfaces
            .iter()
            .filter(|interface| crate::backend::matches(interface, model))
            .map(anonymous_interface)
            .collect();
        let state = read_json(&device_dir(spec.key).join("state.json"))
            .ok()
            .flatten()
            .unwrap_or_else(|| json!({}));
        let error = state
            .get("error")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| state.get("settings_error").and_then(Value::as_str))
            .unwrap_or("");
        let ui_metadata = json!({
            "dpi_range":spec.dpi_range,"dpi_step":spec.dpi_step,
            "rates":spec.rates,"wired_rates":spec.wired_rates,"sleeps":spec.sleeps,
            "advanced":spec.advanced.iter().map(|setting| json!({"key":setting.key,"choices":setting.choices})).collect::<Vec<_>>()
        });
        devices.push(json!({"key":spec.key,"plugin":plugin,"interfaces":ports,"ui_metadata":ui_metadata,"connection":{"enumerated":!ports.is_empty(),"cached_present":state.get("present").and_then(Value::as_bool),"cached_fresh":state.get("fresh").and_then(Value::as_bool),"error_category":error_category(error)}}));
    }
    write_json(
        path,
        &json!({"schema":1,"appVersion":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"interface_enumeration":enumeration,"devices":devices}),
    )?;
    Ok(OperationReport {
        message: "诊断已导出：仅匿名接口与插件/连接信息，不含路径、串号、SDK 位置或按键宏内容。"
            .into(),
        ..OperationReport::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_preserves_future_fields_but_empty_arrays_replace_old_lists() {
        let mut old = json!({"future":{"token":7},"cached":{"controls":{"bindings":[1,2],"colors":[3],"extension":{"value":9}}}});
        merge(
            &mut old,
            json!({"cached":{"controls":{"bindings":[],"colors":[]}}}),
        );
        assert_eq!(old["future"]["token"], 7);
        assert_eq!(old["cached"]["controls"]["extension"]["value"], 9);
        assert_eq!(old["cached"]["controls"]["bindings"], json!([]));
        assert_eq!(old["cached"]["controls"]["colors"], json!([]));
    }
    #[test]
    fn failed_import_restores_exact_bytes_even_when_failure_follows_a_write() {
        let a = PathBuf::from("first.json");
        let b = PathBuf::from("second.json");
        let before = BTreeMap::from([
            (a.clone(), b"old bytes, even malformed JSON".to_vec()),
            (b.clone(), b"second original".to_vec()),
        ]);
        let mut files = before.clone();
        let mut writes = 0;
        let prepared = vec![
            (a.clone(), Some(before[&a].clone()), json!({"new":1})),
            (b.clone(), Some(before[&b].clone()), json!({"new":2})),
        ];
        let result = commit_records(prepared, |path, bytes| {
            writes += 1;
            if let Some(bytes) = bytes {
                files.insert(path.to_owned(), bytes.to_vec());
            } else {
                files.remove(path);
            }
            if writes == 2 {
                Err("injected post-write error".into())
            } else {
                Ok(())
            }
        });
        assert!(result.unwrap_err().contains("已恢复原本地配置"));
        assert_eq!(files, before);
    }
    #[test]
    fn diagnostics_only_export_anonymous_interface_fields_and_error_categories() {
        let interface = crate::hid::Interface {
            path: r"C:\Users\private-person\serial-secret".into(),
            product: "private-person custom name".into(),
            vid: 0x3554,
            pid: 0xfa09,
            usage_page: 0xff02,
            usage: 1,
            feature_len: 0,
            input_len: 20,
            output_len: 20,
        };
        let output=json!({"interface":anonymous_interface(&interface),"error":error_category(r"SDK failed at C:\Users\private-person\osConfLib.dll serial-secret")}).to_string();
        for forbidden in [
            "private-person",
            "serial-secret",
            "Users",
            "osConfLib",
            "path",
            "product",
        ] {
            assert!(!output.contains(forbidden));
        }
        assert!(output.contains("feature_length"));
    }
    #[test]
    fn legacy_configuration_requires_matching_device_and_preserves_extensions() {
        let migrated = normalize(
            json!({"schema":0,"device_key":"hecate-g2-pro","values":{"EqEnabled":1},"future":77}),
            "hecate-g2-pro",
        )
        .unwrap();
        assert_eq!(migrated["schema"], 1);
        assert_eq!(migrated["future"], 77);
        assert_eq!(migrated["cached"]["effects"]["values"]["EqEnabled"], 1);
        assert!(normalize(json!({"device":"mchose-g75-pro"}), "hecate-g2-pro").is_err());
    }
}
