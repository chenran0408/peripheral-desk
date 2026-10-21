//! Shared editable controls; each adapter owns its binary encoding.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingAction {
    Disabled,
    Mouse(u8), // 1 left, 2 right, 3 middle, 4 back, 5 forward
    Keyboard { usage: u16, modifiers: u8 },
    Consumer(u16),
    DpiCycle,
    Macro(u16),
    Unknown(Vec<u8>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub control: u16,
    pub layer: u8,
    pub label: String,
    pub action: BindingAction,
    pub editable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroEvent {
    Key { usage: u16, pressed: bool },
    Mouse { button: u8, pressed: bool },
    Wheel(i8),
    Delay(u16),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroMode {
    Once,
    WhileHeld,
    Toggle,
    Repeat(u16),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroDefinition {
    pub name: String,
    pub mode: MacroMode,
    pub events: Vec<MacroEvent>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyColor {
    pub control: u16,
    pub rgb: [u8; 3],
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ControlState {
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub binding_types: Vec<String>,
    pub colors: Vec<KeyColor>,
    #[serde(default)]
    pub macros: std::collections::BTreeMap<u16, MacroDefinition>,
    pub macro_supported: bool,
    pub macro_max_events: usize,
    pub notice: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ControlAction {
    Bind {
        control: u16,
        layer: u8,
        action: BindingAction,
    },
    Macro {
        control: u16,
        definition: MacroDefinition,
    },
    Colors(Vec<KeyColor>),
}

#[cfg(feature = "driver-runtime")]
#[path = "controls_runtime.rs"]
mod runtime;
#[cfg(feature = "driver-runtime")]
pub use runtime::{apply, read};
#[cfg(not(feature = "driver-runtime"))]
pub fn read(model: usize) -> crate::protocol::Result<ControlState> {
    let spec = crate::devices::REGISTRY.get(model).ok_or("设备编号无效")?;
    serde_json::from_value(crate::plugin_manager::invoke(
        spec.key,
        serde_json::json!({"op":"controls-read"}),
    )?)
    .map_err(|e| format!("驱动返回键位格式错误：{e}"))
}
#[cfg(not(feature = "driver-runtime"))]
pub fn apply(model: usize, action: &ControlAction) -> crate::protocol::Result<ControlState> {
    let spec = crate::devices::REGISTRY.get(model).ok_or("设备编号无效")?;
    serde_json::from_value(crate::plugin_manager::invoke(
        spec.key,
        serde_json::json!({"op":"controls-apply","action":action}),
    )?)
    .map_err(|e| format!("驱动返回键位格式错误：{e}"))
}
