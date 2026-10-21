use crate::{audio_fx, model::Tuning, protocol::Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioProfile {
    schema: u32,
    device: String,
    values: BTreeMap<Tuning, u16>,
}
pub fn validate_audio_values(values: &BTreeMap<Tuning, u16>) -> Result<()> {
    if values.is_empty() {
        return Err("音效配置没有可用参数".into());
    }
    for (key, value) in values {
        let spec = audio_fx::spec(*key).ok_or_else(|| format!("配置包含非音效参数：{key:?}"))?;
        if *value > spec.max {
            return Err(format!("{key:?} 超出允许范围"));
        }
    }
    Ok(())
}
fn parse_audio(bytes: &[u8]) -> Result<BTreeMap<Tuning, u16>> {
    let profile: AudioProfile =
        serde_json::from_slice(bytes).map_err(|e| format!("无法读取音效配置：{e}"))?;
    if profile.schema != 1 {
        return Err("该音效配置版本尚不支持".into());
    }
    if profile.device != "hecate-g2-pro" {
        return Err("该配置不属于 G2 Pro，已拒绝写入".into());
    }
    validate_audio_values(&profile.values)?;
    Ok(profile.values)
}
pub fn load_audio(path: &Path) -> Result<BTreeMap<Tuning, u16>> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("音效配置文件过大".into());
    }
    parse_audio(&bytes)
}
pub fn export_audio(path: &Path) -> Result<()> {
    let values = audio_fx::query(None)?.values;
    validate_audio_values(&values)?;
    let profile = AudioProfile {
        schema: 1,
        device: "hecate-g2-pro".into(),
        values,
    };
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(path, &bytes)
}

/// Local EQ library. Names are display text; all records share one fixed file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqPreset {
    pub id: u64,
    pub name: String,
    pub values: BTreeMap<Tuning, u16>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqLibrary {
    schema: u32,
    device: String,
    pub presets: Vec<EqPreset>,
    pub last_applied: Option<u64>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
fn eq_library_path() -> std::path::PathBuf {
    crate::backend::data_path().with_file_name("eq-presets.json")
}
fn checked_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err("名称需为 1～32 个字符，不能包含控制字符".into());
    }
    Ok(name.into())
}
fn eq_values(values: &BTreeMap<Tuning, u16>) -> Result<BTreeMap<Tuning, u16>> {
    let mut result = BTreeMap::new();
    for key in
        std::iter::once(Tuning::EqEnabled).chain(audio_fx::EQ_BANDS.iter().map(|(key, _)| *key))
    {
        let value = *values
            .get(&key)
            .ok_or("未读到完整十段 EQ，请先连接音效组件并刷新耳机")?;
        result.insert(key, value);
    }
    validate_audio_values(&result)?;
    Ok(result)
}
fn validate_eq_library(library: &EqLibrary) -> Result<()> {
    if library.schema != 1 || library.device != "hecate-g2-pro" || library.presets.len() > 64 {
        return Err("本地 EQ 预设库版本或结构不匹配".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for preset in &library.presets {
        if preset.id == 0
            || !ids.insert(preset.id)
            || !names.insert(checked_name(&preset.name)?.to_lowercase())
        {
            return Err("本地 EQ 预设库包含重复名称或编号".into());
        }
        if eq_values(&preset.values)? != preset.values {
            return Err("EQ 预设含有其他音效，已拒绝读取".into());
        }
    }
    if library.last_applied.is_some_and(|id| !ids.contains(&id)) {
        return Err("本地 EQ 预设库的应用记录无效".into());
    }
    Ok(())
}
fn default_eq_library() -> EqLibrary {
    let curves = [
        ("平直", [0; 10]),
        ("人声清晰", [-3, -2, -1, 0, 2, 3, 2, 1, 0, -1]),
        ("低音增强", [5, 4, 3, 1, 0, -1, -1, 0, 1, 1]),
        ("游戏细节", [-2, -2, -1, 0, 1, 3, 4, 2, 0, -1]),
    ];
    EqLibrary {
        schema: 1,
        device: "hecate-g2-pro".into(),
        presets: curves
            .into_iter()
            .enumerate()
            .map(|(index, (name, gains))| {
                let mut values = BTreeMap::from([(Tuning::EqEnabled, 1)]);
                for ((key, _), gain) in audio_fx::EQ_BANDS.iter().zip(gains) {
                    values.insert(*key, (gain + 20) as u16);
                }
                EqPreset {
                    id: index as u64 + 1,
                    name: name.into(),
                    values,
                    extra: BTreeMap::new(),
                }
            })
            .collect(),
        last_applied: None,
        extra: BTreeMap::new(),
    }
}
fn save_eq_library(library: &EqLibrary) -> Result<()> {
    validate_eq_library(library)?;
    crate::storage::atomic_write(
        &eq_library_path(),
        &serde_json::to_vec_pretty(library).map_err(|e| e.to_string())?,
    )
}
pub fn list_eq_presets() -> Result<EqLibrary> {
    let path = eq_library_path();
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let library = default_eq_library();
            save_eq_library(&library)?;
            return Ok(library);
        }
        Err(error) => return Err(format!("读取 EQ 预设库失败：{error}")),
    };
    let mut bytes = Vec::new();
    file.take(262145)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 262144 {
        return Err("EQ 预设库文件过大".into());
    }
    let library: EqLibrary =
        serde_json::from_slice(&bytes).map_err(|e| format!("EQ 预设库损坏：{e}"))?;
    validate_eq_library(&library)?;
    Ok(library)
}
fn available_name(library: &EqLibrary, name: &str, except: Option<u64>) -> Result<String> {
    let name = checked_name(name)?;
    if library.presets.iter().any(|preset| {
        Some(preset.id) != except && preset.name.to_lowercase() == name.to_lowercase()
    }) {
        return Err("已有同名预设，请换一个名称".into());
    }
    Ok(name)
}
pub fn save_current_eq(name: &str) -> Result<u64> {
    let mut library = list_eq_presets()?;
    if library.presets.len() >= 64 {
        return Err("最多可保存 64 套 EQ 预设".into());
    }
    let name = available_name(&library, name, None)?;
    let values = eq_values(&audio_fx::query(None)?.values)?;
    let id = library
        .presets
        .iter()
        .map(|preset| preset.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("预设编号已用完")?;
    library.presets.push(EqPreset {
        id,
        name,
        values,
        extra: BTreeMap::new(),
    });
    // The saved curve was read from the device, but may change outside this tool.
    library.last_applied = Some(id);
    save_eq_library(&library)?;
    Ok(id)
}
pub fn apply_eq_preset(id: u64) -> Result<()> {
    let mut library = list_eq_presets()?;
    let preset = library
        .presets
        .iter()
        .find(|preset| preset.id == id)
        .ok_or("预设已被移除，请刷新列表")?;
    audio_fx::apply_profile(&preset.values)?;
    library.last_applied = Some(id);
    save_eq_library(&library).map_err(|e| format!("EQ 已应用，但保存应用记录失败：{e}"))
}
pub fn rename_eq_preset(id: u64, name: &str) -> Result<()> {
    let mut library = list_eq_presets()?;
    let name = available_name(&library, name, Some(id))?;
    library
        .presets
        .iter_mut()
        .find(|preset| preset.id == id)
        .ok_or("未找到该预设")?
        .name = name;
    save_eq_library(&library)
}
pub fn delete_eq_preset(id: u64) -> Result<()> {
    let mut library = list_eq_presets()?;
    let before = library.presets.len();
    library.presets.retain(|preset| preset.id != id);
    if library.presets.len() == before {
        return Err("未找到该预设".into());
    }
    if library.last_applied == Some(id) {
        library.last_applied = None;
    }
    save_eq_library(&library)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eq_extensions_survive_rename_and_deletion_does_not_resurrect_records() {
        let mut original = serde_json::to_value(default_eq_library()).unwrap();
        original["futureLibrary"] = serde_json::json!({"version":7});
        original["presets"][0]["futureCurve"] = serde_json::json!({"gainMode":"keep"});
        let mut library: EqLibrary = serde_json::from_value(original).unwrap();
        let id = library.presets[0].id;
        library.presets[0].name = "新的名称".into();
        let renamed = serde_json::to_value(&library).unwrap();
        assert_eq!(renamed["futureLibrary"]["version"], 7);
        assert_eq!(renamed["presets"][0]["futureCurve"]["gainMode"], "keep");
        library.presets.retain(|preset| preset.id != id);
        let deleted = serde_json::to_value(&library).unwrap();
        assert_eq!(deleted["presets"].as_array().unwrap().len(), 3);
        assert!(!deleted["presets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|preset| preset["id"] == id));
        assert_eq!(deleted["futureLibrary"]["version"], 7);
    }
    #[test]
    fn eq_library_keeps_only_eq_and_rejects_duplicate_records() {
        let mut library = default_eq_library();
        validate_eq_library(&library).unwrap();
        library.presets[0].values.insert(Tuning::MicNoise, 1);
        assert!(validate_eq_library(&library).is_err());
        library.presets[0].values.remove(&Tuning::MicNoise);
        library.presets[1].id = library.presets[0].id;
        assert!(validate_eq_library(&library).is_err());
        assert!(checked_name("\n").is_err());
    }
    #[test]
    fn profiles_reject_wrong_devices_versions_and_out_of_range_audio() {
        let mut p = AudioProfile {
            schema: 1,
            device: "hecate-g2-pro".into(),
            values: BTreeMap::from([(Tuning::Eq30, 20), (Tuning::MicNoise, 1)]),
        };
        assert_eq!(
            parse_audio(&serde_json::to_vec(&p).unwrap()).unwrap(),
            p.values
        );
        p.values.insert(Tuning::Eq30, 41);
        assert!(parse_audio(&serde_json::to_vec(&p).unwrap()).is_err());
        p.values.insert(Tuning::Eq30, 20);
        p.values.insert(Tuning::Debounce, 8);
        assert!(parse_audio(&serde_json::to_vec(&p).unwrap()).is_err());
        p.values.remove(&Tuning::Debounce);
        p.device = "mchose-a5-ultra".into();
        assert!(parse_audio(&serde_json::to_vec(&p).unwrap()).is_err());
        p.device = "hecate-g2-pro".into();
        p.schema = 2;
        assert!(parse_audio(&serde_json::to_vec(&p).unwrap()).is_err());
    }
}
