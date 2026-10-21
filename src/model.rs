//! Shared, serializable device state and setting commands.
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub key: String,
    pub present: bool,
    pub fresh: bool,
    pub percent: Option<u8>,
    pub charging: Option<bool>,
    pub wired: bool,
    pub light: Option<u8>,
    pub dpi: Option<u16>,
    pub rate: Option<u16>,
    pub sleep_seconds: Option<u16>,
    pub updated: u64,
    pub identity: String,
    pub error: String,
    pub settings_error: String,
    pub alerted: bool,
    #[serde(default)]
    pub tuning: std::collections::BTreeMap<Tuning, u16>,
    #[serde(default)]
    pub dpi_stages: Vec<u16>,
    #[serde(default)]
    pub active_stage: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Dpi(u16),
    Rate(u16),
    Sleep(u16),
    Light(u8),
    Tune(Tuning, u16),
    Stage(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tuning {
    EqEnabled,
    Eq30,
    Eq60,
    Eq120,
    Eq250,
    Eq500,
    Eq1k,
    Eq2k,
    Eq4k,
    Eq8k,
    Eq16k,
    SurroundEnabled,
    SurroundRoom,
    SmartVolumeEnabled,
    SmartVolumeLevel,
    BrillianceEnabled,
    BrillianceLevel,
    BassEnabled,
    BassLevel,
    ClarityEnabled,
    ClarityLevel,
    MicNoise,
    ChatNoise,
    VoiceEnabled,
    VoiceMode,
    GameVolume,
    ChatVolume,
    MicVolume,
    MicMute,
    LowPower,
    DpiStages,
    SensorRotation,
    EsportsMode,
    Debounce,
    Lod,
    Ripple,
    Angle,
    Motion,
    Brightness,
    MultiColor,
    LightColor,
    Speed,
    WinLock,
    MacMode,
    Latency,
}
impl Action {
    pub fn same_setting(self, other: Self) -> bool {
        match (self, other) {
            (Self::Tune(a, _), Self::Tune(b, _)) => a == b,
            _ => std::mem::discriminant(&self) == std::mem::discriminant(&other),
        }
    }
}
