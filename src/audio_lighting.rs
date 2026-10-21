//! Serializable G2 Pro lighting model and driver dispatch.
#[cfg(not(feature = "driver-runtime"))]
use crate::protocol::Result;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightingMode {
    Static,
    Breathing,
    Rainbow,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightingConfig {
    pub enabled: bool,
    pub mode: LightingMode,
    pub rgb: [u8; 3],
    pub speed: u8,
}
impl Default for LightingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: LightingMode::Static,
            rgb: [4, 193, 255],
            speed: 5,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightingState {
    /// Confirmed through the device's ED/0B feature reply.
    pub enabled: bool,
    /// Last successfully sent by this tool; never claimed as device readback.
    pub saved: Option<LightingConfig>,
    pub notice: String,
}

#[cfg(feature = "driver-runtime")]
#[path = "audio_lighting_runtime.rs"]
mod runtime;
#[cfg(feature = "driver-runtime")]
pub use runtime::{apply, read, set_enabled};
#[cfg(not(feature = "driver-runtime"))]
pub fn read() -> Result<LightingState> {
    serde_json::from_value(crate::plugin_manager::invoke(
        "hecate-g2-pro",
        serde_json::json!({"op":"lighting-read"}),
    )?)
    .map_err(|e| format!("驱动返回灯光格式错误：{e}"))
}
#[cfg(not(feature = "driver-runtime"))]
pub fn apply(config: &LightingConfig) -> Result<LightingState> {
    serde_json::from_value(crate::plugin_manager::invoke(
        "hecate-g2-pro",
        serde_json::json!({"op":"lighting-apply","config":config}),
    )?)
    .map_err(|e| format!("驱动返回灯光格式错误：{e}"))
}
/// Change only the hardware switch, preserving the device's unknown palette.
#[cfg(not(feature = "driver-runtime"))]
pub fn set_enabled(enabled: bool) -> Result<LightingState> {
    serde_json::from_value(crate::plugin_manager::invoke(
        "hecate-g2-pro",
        serde_json::json!({"op":"lighting-switch","enabled":enabled}),
    )?)
    .map_err(|e| format!("驱动返回灯光格式错误：{e}"))
}
