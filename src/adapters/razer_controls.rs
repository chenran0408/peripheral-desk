//! DeathAdder V3 Pro Standard-layer controls.
//!
//! Class 02 / commands 8C and 0C are independently encoded from the public
//! OpenMouse protocol and openrazer-win observations; neither source verifies
//! these commands on 00B7. A matching primary-button response is required before
//! any slot becomes editable. Read-back confirms firmware state, not physical
//! output or retention across power loss. No onboard macro upload is claimed.
use super::mouse::Channel;
use crate::{
    controls::{Binding, BindingAction, ControlAction, ControlState},
    protocol::Result,
};

const SLOTS: [(u8, &str); 5] = [
    (1, "左键"),
    (2, "右键"),
    (3, "滚轮中键"),
    (4, "侧键后退"),
    (5, "侧键前进"),
];
const NOTICE: &str = "仅开放原始标记为 0 的已知映射槽；本机已验证前进侧键。标记为 1 的槽与左键保持只读，不会清零未知标记。板载宏未确认，可在已开放侧键使用需常驻的软件宏。";

fn supported(channel: &Channel) -> Result<()> {
    if channel.interface.vid == 0x1532 && matches!(channel.interface.pid, 0x00b6 | 0x00b7) {
        Ok(())
    } else {
        Err("此改键适配仅用于炼狱蝰蛇 V3 专业版 1532:00B6/00B7".into())
    }
}

/// Reject an unsupported transport's successful-but-zero response; unknown
/// action types remain opaque, and never become editable through decoding.
fn decode(control: u8, body: &[u8]) -> Result<BindingAction> {
    if body.len() != 10 || body[..2] != [1, control] || ![0, 1].contains(&body[2]) || body[4] > 5 {
        return Err(format!(
            "按键 {control} 的配置回复不匹配，未确认此固件支持改键"
        ));
    }
    let action = match (body[3], body[4]) {
        (0, 0) if body[5..].iter().all(|&b| b == 0) => BindingAction::Disabled,
        (1, 1) if (1..=5).contains(&body[5]) && body[6..].iter().all(|&b| b == 0) => {
            BindingAction::Mouse(body[5])
        }
        (2, 2) if valid_key(body[6] as u16, body[5]) && body[7..].iter().all(|&b| b == 0) => {
            BindingAction::Keyboard {
                usage: body[6] as u16,
                modifiers: body[5],
            }
        }
        _ => BindingAction::Unknown(body.to_vec()),
    };
    Ok(action)
}

fn valid_key(usage: u16, modifiers: u8) -> bool {
    (4..=0xe7).contains(&usage) || (usage == 0 && modifiers != 0)
}

fn arguments(control: u16, layer: u8, action: &BindingAction) -> Result<[u8; 10]> {
    if layer != 0 || !(2..=5).contains(&control) {
        return Err("只支持标准层右键、中键和两个侧键；左键保持固定".into());
    }
    let mut args = [1, control as u8, 0, 0, 0, 0, 0, 0, 0, 0];
    match action {
        BindingAction::Disabled => {}
        BindingAction::Mouse(button) if (1..=5).contains(button) => {
            args[3..6].copy_from_slice(&[1, 1, *button]);
        }
        BindingAction::Keyboard { usage, modifiers } if valid_key(*usage, *modifiers) => {
            args[3..7].copy_from_slice(&[2, 2, *modifiers, *usage as u8]);
        }
        _ => return Err("此雷蛇适配当前仅支持鼠标功能、键盘按键与禁用；未知配置保持只读".into()),
    }
    Ok(args)
}

fn read_slot(channel: &Channel, control: u8) -> Result<(Vec<u8>, BindingAction)> {
    let body = channel.razer_controls(0x8c, &[1, control, 0])?;
    let action = decode(control, &body)?;
    Ok((body, action))
}

fn primary(channel: &Channel) -> Result<()> {
    supported(channel)?;
    let (_, action) = read_slot(channel, 1)?;
    if action != BindingAction::Mouse(1) {
        return Err("左键配置未返回有效主点击，保留设备原设置并关闭改键".into());
    }
    Ok(())
}

pub fn read(channel: &Channel) -> Result<ControlState> {
    primary(channel)?;
    let mut state = ControlState {
        binding_types: ["mouse", "keyboard", "disabled"].map(String::from).to_vec(),
        notice: NOTICE.into(),
        ..ControlState::default()
    };
    for (control, label) in SLOTS {
        // The first response already established the primary-click anchor.
        let result = if control == 1 {
            Ok((false, BindingAction::Mouse(1)))
        } else {
            read_slot(channel, control).map(|(body, action)| (body[2] == 0, action))
        };
        let (action, editable) = match result {
            Ok((confirmed, action)) => {
                let editable =
                    confirmed && control != 1 && !matches!(action, BindingAction::Unknown(_));
                (action, editable)
            }
            Err(error) => {
                state.notice.push_str(&format!(" {label}：{error}"));
                (BindingAction::Unknown(Vec::new()), false)
            }
        };
        state.bindings.push(Binding {
            control: control as u16,
            layer: 0,
            label: label.into(),
            action,
            editable,
        });
    }
    Ok(state)
}

pub fn apply(channel: &Channel, action: &ControlAction) -> Result<()> {
    let ControlAction::Bind {
        control,
        layer,
        action,
    } = action
    else {
        return Err("此型号尚未确认板载宏写入协议；当前仅开放普通改键".into());
    };
    let mut args = arguments(*control, *layer, action)?;
    primary(channel)?;
    let (baseline, current) = read_slot(channel, *control as u8)?;
    if matches!(current, BindingAction::Unknown(_)) {
        return Err("原按键使用未知配置，保持只读以免覆盖官方设置".into());
    }
    if current == *action {
        return Ok(());
    }
    if baseline[2] != 0 {
        return Err("该槽原始标记尚未确认可写，已保持只读；不会清零或覆盖未知标记".into());
    }
    // 00B7 reports an opaque third byte of 0 or 1; echo its actual value.
    // It is not a verified Hypershift selector on this model.
    args[2] = baseline[2];
    let result = channel.razer_controls(0x0c, &args).and_then(|_| {
        let (actual, _) = read_slot(channel, *control as u8)?;
        if actual == args {
            Ok(())
        } else {
            Err("设备读回的按键配置与请求不一致".into())
        }
    });
    if let Err(error) = result {
        // A transport failure may occur after the device accepted the write.
        // Restore the complete raw baseline once; never retry a setting write.
        let restored = channel.razer_controls(0x0c, &baseline).and_then(|_| {
            let (actual, _) = read_slot(channel, *control as u8)?;
            if actual == baseline {
                Ok(())
            } else {
                Err("恢复后读回仍不一致".into())
            }
        });
        return Err(match restored {
            Ok(()) => format!("改键失败：{error}；已恢复原按键配置"),
            Err(restore) => format!("改键失败：{error}；原配置恢复未能确认：{restore}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_reply_does_not_create_disabled_buttons() {
        assert!(decode(4, &[0; 10]).is_err());
        assert!(decode(4, &[1, 5, 0, 1, 1, 5, 0, 0, 0, 0]).is_err());
        assert_eq!(
            decode(4, &[1, 4, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            BindingAction::Disabled
        );
    }
    #[test]
    fn unknown_action_stays_raw_and_main_click_stays_fixed() {
        let raw = [1, 4, 0, 0x55, 1, 3, 0, 0, 0, 0];
        assert_eq!(
            decode(4, &raw).unwrap(),
            BindingAction::Unknown(raw.to_vec())
        );
        assert!(arguments(1, 0, &BindingAction::Mouse(2)).is_err());
        assert!(arguments(4, 1, &BindingAction::Mouse(2)).is_err());
        assert!(arguments(4, 0, &BindingAction::Unknown(raw.to_vec())).is_err());
    }
    #[test]
    fn keyboard_and_mouse_payloads_preserve_exact_fields() {
        let action = BindingAction::Keyboard {
            usage: 0x06,
            modifiers: 1,
        };
        let args = arguments(4, 0, &action).unwrap();
        assert_eq!(args, [1, 4, 0, 2, 2, 1, 6, 0, 0, 0]);
        assert_eq!(decode(4, &args).unwrap(), action);
        assert_eq!(
            arguments(5, 0, &BindingAction::Mouse(4)).unwrap(),
            [1, 5, 0, 1, 1, 4, 0, 0, 0, 0]
        );
        assert!(arguments(
            4,
            0,
            &BindingAction::Keyboard {
                usage: 256,
                modifiers: 0
            }
        )
        .is_err());
    }
}
