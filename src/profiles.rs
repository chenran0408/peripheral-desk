use crate::{audio_fx, model::Tuning, protocol::Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
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
    use std::io::Read;
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
#[cfg(test)]
mod tests {
    use super::*;
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
