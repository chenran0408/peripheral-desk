//! G75 Pro structured controls derived from the official MCHOSE HUB schemas.
//! Unknown mappings, matrix holes and trailing bytes are preserved verbatim.
use crate::{
    controls::{Binding, BindingAction, ControlAction, ControlState, KeyColor},
    keyboard::Keyboard,
    protocol::Result,
};

const KEYS: [&str; 96] = [
    "Esc",
    "backquote",
    "Tab",
    "Caps",
    "Left_Shift",
    "Left_Ctrl",
    "F1",
    "one",
    "Q",
    "A",
    "Z",
    "Left_Win",
    "F2",
    "two",
    "W",
    "S",
    "X",
    "Left_Alt",
    "F3",
    "three",
    "E",
    "D",
    "C",
    "space1",
    "F4",
    "four",
    "R",
    "F",
    "V",
    "space2",
    "F5",
    "five",
    "T",
    "G",
    "B",
    "Space",
    "F6",
    "six",
    "Y",
    "H",
    "N",
    "space3",
    "F7",
    "seven",
    "U",
    "J",
    "M",
    "space4",
    "F8",
    "eight",
    "I",
    "K",
    "comma",
    "Fn",
    "F9",
    "nine",
    "O",
    "L",
    "dot",
    "Right_Ctrl",
    "F10",
    "zero",
    "P",
    "semicolon",
    "slash",
    "space5",
    "F11",
    "transverse",
    "left_bracket",
    "singlequotes",
    "space6",
    "space7",
    "F12",
    "equal",
    "right_bracket",
    "noChange0",
    "space8",
    "space9",
    "Insert",
    "Backspace",
    "backslash",
    "Enter",
    "Right_Shift",
    "left",
    "space10",
    "space11",
    "space12",
    "space13",
    "up",
    "down",
    "Delete",
    "Home",
    "PgUp",
    "PgDn",
    "End",
    "right",
];
const MACRO_NOTICE: &str =
    "设备宏暂未开放：官方驱动依赖本地宏目录，尚无完整设备宏备份与读回协议，不能安全覆盖现有宏。";
fn real_key(index: usize) -> bool {
    index < KEYS.len() && !KEYS[index].starts_with("space") && !KEYS[index].starts_with("noChange")
}
fn validate_bindings(bytes: &[u8]) -> Result<()> {
    // The actual receiver returns 126 slots (504 bytes), while the official
    // G75 layout exposes only its first 96 slots. Preserve the hidden tail.
    if bytes.len() != 504 {
        return Err("G75 键位配置长度不匹配，已禁止写入".into());
    }
    Ok(())
}
fn validate_colors(bytes: &[u8]) -> Result<()> {
    if bytes.len() != 378 {
        return Err("G75 逐键灯配置长度不匹配，已禁止写入".into());
    }
    Ok(())
}
fn decode(raw: &[u8]) -> BindingAction {
    match raw {
        [0, 0, 0, 0] => BindingAction::Disabled,
        [0, mods, a, b] if *a == 0 || *b == 0 => BindingAction::Keyboard {
            usage: u16::from((*a).max(*b)),
            modifiers: *mods,
        },
        [1, button, 1, 0] if (1..=5).contains(button) => BindingAction::Mouse(*button),
        [2, 0, high, low] => BindingAction::Consumer(u16::from_be_bytes([*high, *low])),
        [3, _, 1, index] => BindingAction::Macro(u16::from(*index)),
        _ => BindingAction::Unknown(raw.to_vec()),
    }
}
fn encode(action: &BindingAction) -> Result<[u8; 4]> {
    Ok(match action {
        BindingAction::Disabled => [0; 4],
        BindingAction::Keyboard { usage, modifiers } if *usage <= 231 => {
            if (224..=231).contains(usage) {
                [0, *modifiers | (1 << (*usage - 224)), 0, 0]
            } else {
                [0, *modifiers, 0, *usage as u8]
            }
        }
        BindingAction::Mouse(button) if (1..=5).contains(button) => [1, *button, 1, 0],
        BindingAction::Consumer(usage) => {
            let [high, low] = usage.to_be_bytes();
            [2, 0, high, low]
        }
        BindingAction::Macro(_) => return Err(MACRO_NOTICE.into()),
        _ => return Err("G75 不支持该键位动作".into()),
    })
}
fn edit_binding(bytes: &mut [u8], control: u16, action: &BindingAction) -> Result<()> {
    validate_bindings(bytes)?;
    let index = control as usize;
    if !real_key(index) || KEYS[index] == "Fn" {
        return Err("该键位是空槽或保留的 Fn 键，不能修改".into());
    }
    let raw = encode(action)?;
    bytes[index * 4..index * 4 + 4].copy_from_slice(&raw);
    Ok(())
}
fn edit_colors(bytes: &mut [u8], colors: &[KeyColor]) -> Result<()> {
    validate_colors(bytes)?;
    if colors.is_empty() {
        return Err("请选择至少一个按键颜色".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for color in colors {
        if !real_key(color.control as usize) || !seen.insert(color.control) {
            return Err("逐键灯包含无效或重复键位".into());
        }
    }
    for color in colors {
        for channel in 0..3 {
            bytes[channel * 126 + color.control as usize] = color.rgb[channel];
        }
    }
    Ok(())
}
pub fn read(keyboard: &Keyboard) -> Result<ControlState> {
    keyboard.verify_model()?;
    let mut state = ControlState {
        binding_types: ["mouse", "keyboard", "consumer", "disabled"]
            .map(String::from)
            .to_vec(),
        notice: MACRO_NOTICE.into(),
        ..ControlState::default()
    };
    let mut errors = Vec::new();
    for layer in 0..=2 {
        match keyboard.read_block_layer(0x41, layer).and_then(|bytes| {
            validate_bindings(&bytes)?;
            Ok(bytes)
        }) {
            Ok(bytes) => {
                for (index, key) in KEYS
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| real_key(*index))
                {
                    state.bindings.push(Binding {
                        control: index as u16,
                        layer,
                        label: label(key).into(),
                        action: decode(&bytes[index * 4..index * 4 + 4]),
                        editable: *key != "Fn",
                    });
                }
            }
            Err(error) => errors.push(format!("第{layer}层键位：{error}")),
        }
    }
    match keyboard.read_block_layer(0x42, 0).and_then(|bytes| {
        validate_colors(&bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) => {
            for index in 0..KEYS.len() {
                if real_key(index) {
                    state.colors.push(KeyColor {
                        control: index as u16,
                        rgb: [bytes[index], bytes[126 + index], bytes[252 + index]],
                    });
                }
            }
        }
        Err(error) => errors.push(format!("逐键灯：{error}")),
    }
    if !errors.is_empty() {
        state.notice = format!("{} {}", errors.join("；"), MACRO_NOTICE);
    }
    Ok(state)
}
pub fn apply(keyboard: &Keyboard, action: &ControlAction) -> Result<()> {
    keyboard.verify_model()?;
    match action {
        ControlAction::Bind {
            control,
            layer,
            action,
        } => {
            if *layer > 2 {
                return Err("键位层超出范围".into());
            }
            let original = keyboard.read_block_layer(0x41, *layer)?;
            let mut desired = original.clone();
            edit_binding(&mut desired, *control, action)?;
            keyboard.write_block_layer(1, 0x41, *layer, &desired, &original)
        }
        ControlAction::Colors(colors) => {
            let original = keyboard.read_block_layer(0x42, 0)?;
            let mut desired = original.clone();
            edit_colors(&mut desired, colors)?;
            let original_config = keyboard.read_block(0x44)?;
            if original_config.len() != 128 || original_config[126..] != [0x5a, 0xa5] {
                return Err("G75 性能配置结构不匹配".into());
            }
            let mut desired_config = original_config.clone();
            desired_config[9] = 1;
            desired_config[10] = 19;
            keyboard.write_block_layer(2, 0x42, 0, &desired, &original)?;
            if let Err(error) =
                keyboard.write_block_layer(4, 0x44, 0, &desired_config, &original_config)
            {
                return match keyboard.write_block_layer(2, 0x42, 0, &original, &desired) {
                    Ok(()) => Err(format!("{error}；已恢复原逐键颜色")),
                    Err(restore) => Err(format!("{error}；原逐键颜色恢复失败：{restore}")),
                };
            }
            Ok(())
        }
        ControlAction::Macro { .. } => Err(MACRO_NOTICE.into()),
    }
}
fn label(key: &str) -> &str {
    match key {
        "backquote" => "` ~",
        "Caps" => "Caps Lock",
        "Left_Shift" => "左 Shift",
        "Right_Shift" => "右 Shift",
        "Left_Ctrl" => "左 Ctrl",
        "Right_Ctrl" => "右 Ctrl",
        "Left_Alt" => "左 Alt",
        "Left_Win" => "左 Win",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        "zero" => "0",
        "comma" => ",",
        "dot" => ".",
        "semicolon" => ";",
        "slash" => "/",
        "transverse" => "-",
        "left_bracket" => "[",
        "right_bracket" => "]",
        "singlequotes" => "'",
        "equal" => "=",
        "backslash" => "\\",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        other => other,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_edits_preserve_matrix_holes_unknown_bindings_and_padding() {
        let before = vec![0x5a; 504];
        let mut after = before.clone();
        edit_binding(
            &mut after,
            9,
            &BindingAction::Keyboard {
                usage: 6,
                modifiers: 1,
            },
        )
        .unwrap();
        assert_eq!(&after[36..40], &[0, 1, 0, 6]);
        after[36..40].copy_from_slice(&before[36..40]);
        assert_eq!(after, before);
        for control in [23, 53, 75, 96] {
            assert!(edit_binding(&mut after, control, &BindingAction::Disabled).is_err());
            assert_eq!(after, before);
        }
        assert_eq!(
            decode(&[0, 1, 6, 0]),
            BindingAction::Keyboard {
                usage: 6,
                modifiers: 1
            }
        );
        assert_eq!(
            decode(&[7, 0, 0, 4]),
            BindingAction::Unknown(vec![7, 0, 0, 4])
        );
        assert!(encode(&BindingAction::Macro(0)).is_err());
    }
    #[test]
    fn rgb_edits_preserve_other_keys_and_all_three_plane_tails() {
        let before = vec![0x5a; 378];
        let mut after = before.clone();
        edit_colors(
            &mut after,
            &[KeyColor {
                control: 9,
                rgb: [1, 2, 3],
            }],
        )
        .unwrap();
        for (channel, value) in [1, 2, 3].iter().enumerate() {
            assert_eq!(after[channel * 126 + 9], *value);
            after[channel * 126 + 9] = 0x5a;
        }
        assert_eq!(after, before);
        assert!(edit_colors(
            &mut after,
            &[KeyColor {
                control: 23,
                rgb: [1, 2, 3]
            }]
        )
        .is_err());
        assert_eq!(after, before);
    }
}
