//! Shared audio-effect metadata; native SDK code exists only in driver builds.
use crate::{model::Tuning, protocol::Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub struct FxSpec {
    pub key: Tuning,
    pub channel: usize,
    pub property: &'static str,
    pub max: u16,
    pub offset: i32,
}
pub const SPECS: &[FxSpec] = &[
    FxSpec {
        key: Tuning::EqEnabled,
        channel: 0,
        property: "Enable_EQ_GFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::Eq30,
        channel: 0,
        property: "EQ_Slider1",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq60,
        channel: 0,
        property: "EQ_Slider2",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq120,
        channel: 0,
        property: "EQ_Slider3",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq250,
        channel: 0,
        property: "EQ_Slider4",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq500,
        channel: 0,
        property: "EQ_Slider5",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq1k,
        channel: 0,
        property: "EQ_Slider6",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq2k,
        channel: 0,
        property: "EQ_Slider7",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq4k,
        channel: 0,
        property: "EQ_Slider8",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq8k,
        channel: 0,
        property: "EQ_Slider9",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq16k,
        channel: 0,
        property: "EQ_Slider10",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::SurroundEnabled,
        channel: 0,
        property: "Enable_CONLAB_HPSPK_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SurroundRoom,
        channel: 0,
        property: "CONLAB_HPSPK_ROOMSIZE",
        max: 2,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SmartVolumeEnabled,
        channel: 0,
        property: "Enable_ADAPTIVEVOLUME_GFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SmartVolumeLevel,
        channel: 0,
        property: "ADAPTIVEVOLUME_LEVEL",
        max: 100,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BrillianceEnabled,
        channel: 0,
        property: "Enable_AUDIOBRILLIANT_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BrillianceLevel,
        channel: 0,
        property: "AUDIOBRILLIANT_LEVEL",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BassEnabled,
        channel: 0,
        property: "Enable_VIRTUALBASS_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BassLevel,
        channel: 0,
        property: "VIRTUALBASS_Level",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ClarityEnabled,
        channel: 0,
        property: "Enable_VOICECLARITY_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ClarityLevel,
        channel: 0,
        property: "VOICECLARITY_LEVEL",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::MicNoise,
        channel: 2,
        property: "AI_NR_Enable",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ChatNoise,
        channel: 1,
        property: "AI_Playback_NR_Enable",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::VoiceEnabled,
        channel: 2,
        property: "Enable_MAGICVOICE",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::VoiceMode,
        channel: 2,
        property: "MagicVoice_Selection",
        max: 3,
        offset: 0,
    },
];
pub fn spec(key: Tuning) -> Option<&'static FxSpec> {
    SPECS.iter().find(|s| s.key == key)
}
pub fn page(key: Tuning) -> u8 {
    match key {
        Tuning::GameVolume | Tuning::ChatVolume | Tuning::MicVolume | Tuning::MicMute => 0,
        Tuning::EqEnabled
        | Tuning::Eq30
        | Tuning::Eq60
        | Tuning::Eq120
        | Tuning::Eq250
        | Tuning::Eq500
        | Tuning::Eq1k
        | Tuning::Eq2k
        | Tuning::Eq4k
        | Tuning::Eq8k
        | Tuning::Eq16k => 1,
        Tuning::SurroundEnabled
        | Tuning::SurroundRoom
        | Tuning::SmartVolumeEnabled
        | Tuning::SmartVolumeLevel => 2,
        Tuning::BrillianceEnabled
        | Tuning::BrillianceLevel
        | Tuning::BassEnabled
        | Tuning::BassLevel
        | Tuning::ClarityEnabled
        | Tuning::ClarityLevel => 3,
        _ => 4,
    }
}
pub const EQ_BANDS: &[(Tuning, &str)] = &[
    (Tuning::Eq30, "30"),
    (Tuning::Eq60, "60"),
    (Tuning::Eq120, "120"),
    (Tuning::Eq250, "250"),
    (Tuning::Eq500, "500"),
    (Tuning::Eq1k, "1k"),
    (Tuning::Eq2k, "2k"),
    (Tuning::Eq4k, "4k"),
    (Tuning::Eq8k, "8k"),
    (Tuning::Eq16k, "16k"),
];
pub const PAGES: &[&str] = &["音量", "均衡器", "环绕", "增强", "语音"];
#[derive(Serialize, Deserialize)]
struct Config {
    path: PathBuf,
}
fn config_path() -> PathBuf {
    crate::backend::data_path().with_file_name("hecate-sdk.json")
}
pub fn configure(path: &Path) -> Result<()> {
    validate_library(path)?;
    let bytes = serde_json::to_vec_pretty(&Config {
        path: path.to_path_buf(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(config_path().parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(config_path(), bytes).map_err(|e| e.to_string())
}
fn validate_library(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || !path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("osConfLib.dll"))
    {
        return Err("请选择官方安装目录中的 x64/osConfLib.dll".into());
    }
    let data = std::fs::read(path).map_err(|e| format!("无法读取音效组件：{e}"))?;
    let hash = format!("{:x}", Sha256::digest(&data));
    if hash != "576c1306133a33b975d7ecf25a2b7078052b2566e3705715c247d1e1a1ef2947" {
        return Err("该音效组件版本尚未验证，请使用已验证的 C-Media 1.0.8.0 x64 组件".into());
    }
    Ok(())
}
#[cfg(feature = "driver-runtime")]
#[derive(Serialize, Deserialize)]
struct Request {
    path: PathBuf,
    action: Option<(Tuning, u16)>,
    #[serde(default)]
    values: BTreeMap<Tuning, u16>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Reply {
    pub values: BTreeMap<Tuning, u16>,
    pub error: String,
}

#[cfg(feature = "driver-runtime")]
#[path = "audio_fx_runtime.rs"]
mod runtime;
pub fn query(action: Option<(Tuning, u16)>) -> Result<Reply> {
    request(action, BTreeMap::new())
}
pub fn apply_profile(values: &BTreeMap<Tuning, u16>) -> Result<Reply> {
    crate::profiles::validate_audio_values(values)?;
    request(None, values.clone())
}
fn request(action: Option<(Tuning, u16)>, values: BTreeMap<Tuning, u16>) -> Result<Reply> {
    #[cfg(feature = "driver-runtime")]
    {
        runtime::request(action, values)
    }
    #[cfg(not(feature = "driver-runtime"))]
    {
        serde_json::from_value(crate::plugin_manager::invoke(
            "hecate-g2-pro",
            serde_json::json!({"op":"fx-query","action":action,"values":values}),
        )?)
        .map_err(|e| format!("驱动返回音效格式错误：{e}"))
    }
}
#[cfg(feature = "driver-runtime")]
pub(crate) fn run_request(request: serde_json::Value) -> Result<Reply> {
    runtime::run(serde_json::from_value(request).map_err(|e| format!("音效请求无效：{e}"))?)
}
pub fn worker_main() {
    let result = (|| -> Result<Reply> {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16384 {
            return Err("音效请求过大".into());
        }
        let request: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        #[cfg(feature = "driver-runtime")]
        {
            run_request(request)
        }
        #[cfg(not(feature = "driver-runtime"))]
        {
            serde_json::from_value(crate::plugin_manager::invoke(
                "hecate-g2-pro",
                serde_json::json!({"op":"fx-worker","request":request}),
            )?)
            .map_err(|e| e.to_string())
        }
    })();
    let reply = match result {
        Ok(reply) => reply,
        Err(error) => Reply {
            error,
            ..Reply::default()
        },
    };
    if let Ok(bytes) = serde_json::to_vec(&reply) {
        let _ = std::io::stdout().write_all(&bytes);
    }
}
