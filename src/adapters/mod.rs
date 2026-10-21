//! Each protocol implements DeviceSession; the refresh engine and UI stay shared.
#[cfg(feature = "driver-runtime")]
pub mod audio;
#[cfg(feature = "driver-runtime")]
pub mod mchose_controls;
#[cfg(feature = "driver-runtime")]
pub mod mouse;
#[cfg(feature = "driver-runtime")]
pub mod razer_controls;
use crate::{
    devices::REGISTRY,
    hid::Interface,
    model::{Action, State},
    protocol::Result,
};
pub trait DeviceSession {
    fn read(&self) -> Result<State>;
    /// Must validate bounds, preserve unrelated fields and confirm by reading back.
    fn apply(&self, action: Action) -> Result<()>;
}
#[cfg(feature = "driver-runtime")]
struct CompxSession {
    keyboard: crate::keyboard::Keyboard,
    identity: String,
}
#[cfg(feature = "driver-runtime")]
impl DeviceSession for CompxSession {
    fn read(&self) -> Result<State> {
        self.keyboard.read(self.identity.clone())
    }
    fn apply(&self, action: Action) -> Result<()> {
        self.keyboard.apply(action)
    }
}
#[cfg(feature = "driver-runtime")]
pub fn open(model: usize, interface: &Interface) -> Result<Box<dyn DeviceSession>> {
    use crate::devices::Adapter;
    let spec = &REGISTRY[model];
    match spec.adapter {
        Adapter::Razer | Adapter::MchoseMouse => {
            Ok(Box::new(mouse::Channel::new(interface, spec)?))
        }
        Adapter::HecateAudio => Ok(Box::new(audio::AudioSession::open()?)),
        Adapter::CompxKeyboard => {
            let keyboard = crate::keyboard::Keyboard::open(&interface.path)?;
            keyboard.verify_model()?;
            Ok(Box::new(CompxSession {
                keyboard,
                identity: format!("{:04X}:{:04X}", interface.vid, interface.pid),
            }))
        }
        _ => Err("外部设备必须由其独立驱动插件实现".into()),
    }
}

#[cfg(not(feature = "driver-runtime"))]
struct PluginSession {
    key: &'static str,
}
#[cfg(not(feature = "driver-runtime"))]
impl DeviceSession for PluginSession {
    fn read(&self) -> Result<State> {
        serde_json::from_value(crate::plugin_manager::invoke(
            self.key,
            serde_json::json!({"op":"read-state"}),
        )?)
        .map_err(|e| format!("驱动返回状态格式错误：{e}"))
    }
    fn apply(&self, action: Action) -> Result<()> {
        crate::plugin_manager::invoke(
            self.key,
            serde_json::json!({"op":"apply-setting","action":action}),
        )
        .map(|_| ())
    }
}
#[cfg(not(feature = "driver-runtime"))]
pub fn open(model: usize, _interface: &Interface) -> Result<Box<dyn DeviceSession>> {
    let spec = REGISTRY.get(model).ok_or("设备编号无效")?;
    Ok(Box::new(PluginSession { key: spec.key }))
}
