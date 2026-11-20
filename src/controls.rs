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
    UntilOtherKey,
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
    #[serde(default)]
    pub macro_modes: Vec<MacroMode>,
    #[serde(default)]
    pub macro_event_types: Vec<String>,
    #[serde(default)]
    pub macro_name_max_bytes: Option<usize>,
    pub notice: String,
}
impl ControlState {
    pub fn modes(&self) -> &[MacroMode] {
        if self.macro_modes.is_empty() {
            &[MacroMode::Once]
        } else {
            &self.macro_modes
        }
    }
    pub fn supports_event(&self, kind: &str) -> bool {
        if self.macro_event_types.is_empty() {
            matches!(kind, "key" | "delay")
        } else {
            self.macro_event_types.iter().any(|item| item == kind)
        }
    }
    pub fn macro_limit_error(&self, definition: &MacroDefinition) -> Option<String> {
        if !self.modes().contains(&definition.mode) {
            return Some("设备未提供这种播放方式".into());
        }
        let name_limit = self.macro_name_max_bytes.unwrap_or(20);
        if definition.name.trim().is_empty() || definition.name.len() > name_limit {
            return Some(format!("宏名称须为 1 至 {name_limit} 个 UTF-8 字节"));
        }
        if definition.events.is_empty() || definition.events.len() > self.macro_max_events {
            return Some(format!("宏事件须为 1 至 {} 步", self.macro_max_events));
        }
        definition.events.iter().find_map(|event| {
            let kind = match event {
                MacroEvent::Key { .. } => "key",
                MacroEvent::Delay(_) => "delay",
                MacroEvent::Mouse { .. } => "mouse",
                MacroEvent::Wheel(_) => "wheel",
            };
            (!self.supports_event(kind)).then(|| "宏含此设备未提供的事件类型".into())
        })
    }
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
        serde_json::json!({"op":"controls-read","extended_modes":true}),
    )?)
    .map_err(|e| format!("驱动返回键位格式错误：{e}"))
}
#[cfg(not(feature = "driver-runtime"))]
pub fn apply(model: usize, action: &ControlAction) -> crate::protocol::Result<ControlState> {
    let spec = crate::devices::REGISTRY.get(model).ok_or("设备编号无效")?;
    serde_json::from_value(crate::plugin_manager::invoke(
        spec.key,
        serde_json::json!({"op":"controls-apply","extended_modes":true,"action":action}),
    )?)
    .map_err(|e| format!("驱动返回键位格式错误：{e}"))
}
