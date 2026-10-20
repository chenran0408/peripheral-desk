//! Each protocol implements DeviceSession; the refresh engine and UI stay shared.
pub mod audio;
pub mod mouse;
use crate::{
    devices::{Adapter, REGISTRY},
    hid::Interface,
    model::{Action, State},
    protocol::Result,
};
pub trait DeviceSession {
    fn read(&self) -> Result<State>;
    /// Must validate bounds, preserve unrelated fields and confirm by reading back.
    fn apply(&self, action: Action) -> Result<()>;
}
struct CompxSession {
    keyboard: crate::keyboard::Keyboard,
    identity: String,
}
impl DeviceSession for CompxSession {
    fn read(&self) -> Result<State> {
        self.keyboard.read(self.identity.clone())
    }
    fn apply(&self, action: Action) -> Result<()> {
        self.keyboard.apply(action)
    }
}
pub fn open(model: usize, interface: &Interface) -> Result<Box<dyn DeviceSession>> {
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
    }
}
