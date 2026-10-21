use super::*;
use crate::{
    devices::{Adapter, REGISTRY},
    protocol::Result,
};
enum Session {
    Mouse(crate::adapters::mouse::Channel),
    Keyboard(crate::keyboard::Keyboard),
}
fn connect(model: usize) -> Result<Session> {
    let spec = REGISTRY.get(model).ok_or("设备编号无效")?;
    let all = crate::hid::enumerate()?;
    let i = all
        .iter()
        .find(|i| crate::backend::matches(i, model))
        .ok_or("设备尚未连接")?;
    match spec.adapter {
        Adapter::Razer | Adapter::MchoseMouse => Ok(Session::Mouse(
            crate::adapters::mouse::Channel::new(i, spec)?,
        )),
        Adapter::CompxKeyboard => {
            let k = crate::keyboard::Keyboard::open(&i.path)?;
            k.verify_model()?;
            Ok(Session::Keyboard(k))
        }
        _ => Err("该设备没有键位或逐键灯设置".into()),
    }
}
pub fn read(model: usize) -> Result<ControlState> {
    match connect(model)? {
        Session::Mouse(c) => {
            if c.spec.adapter == Adapter::Razer {
                crate::adapters::razer_controls::read(&c)
            } else {
                crate::adapters::mchose_controls::read(&c)
            }
        }
        Session::Keyboard(k) => crate::keyboard_controls::read(&k),
    }
}
pub fn apply(model: usize, action: &ControlAction) -> Result<ControlState> {
    match connect(model)? {
        Session::Mouse(c) => {
            if c.spec.adapter == Adapter::Razer {
                crate::adapters::razer_controls::apply(&c, action)?;
            } else {
                crate::adapters::mchose_controls::apply(&c, action)?;
            }
        }
        Session::Keyboard(k) => crate::keyboard_controls::apply(&k, action)?,
    };
    read(model).map_err(|error| format!("设置已写入并核对，但刷新失败，请重新读取设备：{error}"))
}
