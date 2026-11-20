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
    spec: &'static crate::devices::DeviceSpec,
}
#[cfg(not(feature = "driver-runtime"))]
impl DeviceSession for PluginSession {
    fn read(&self) -> Result<State> {
        if self.spec.switch_only() {
            let reply = crate::plugin_manager::invoke(
                self.spec.key,
                serde_json::json!({"op":"lighting-read"}),
            )?;
            let enabled = reply
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .ok_or("插件未返回灯光开关状态")?;
            return Ok(State {
                key: self.spec.key.into(),
                present: true,
                fresh: true,
                light: Some(u8::from(enabled)),
                updated: crate::backend::now(),
                ..State::default()
            });
        }
        serde_json::from_value(crate::plugin_manager::invoke(
            self.spec.key,
            serde_json::json!({"op":"read-state"}),
        )?)
        .map_err(|e| format!("驱动返回状态格式错误：{e}"))
    }
    fn apply(&self, action: Action) -> Result<()> {
        let request = match action {
            Action::Light(enabled @ 0..=1) if self.spec.switch_only() => {
                serde_json::json!({"op":"lighting-switch","enabled":enabled == 1})
            }
            _ => serde_json::json!({"op":"apply-setting","action":action}),
        };
        crate::plugin_manager::invoke(self.spec.key, request).map(|_| ())
    }
}
#[cfg(not(feature = "driver-runtime"))]
pub fn open(model: usize, _interface: &Interface) -> Result<Box<dyn DeviceSession>> {
    let spec = REGISTRY.get(model).ok_or("设备编号无效")?;
    Ok(Box::new(PluginSession { spec }))
}
