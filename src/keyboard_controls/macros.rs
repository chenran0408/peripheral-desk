//! G75 Pro wireless macro directory. The first LE16 address is the size
//! of the directory, whose entries are (LE16 offset, LE16 record length).
use super::{real_key, validate_bindings, Keyboard, KEYS};
use crate::{
    controls::{ControlState, MacroDefinition, MacroEvent, MacroMode},
    protocol::Result,
};
use std::ops::Range;

const IMAGE_LEN: usize = 1024;
// Each firmware action takes four bytes. Its 19-bit delay can occupy up to
// nine u16 Delay steps in the shared editor.
const MAX_EVENTS: usize = ((IMAGE_LEN - 4 - 2) / 4) * 10;

fn ranges(image: &[u8]) -> Result<Vec<Range<usize>>> {
    let word = |i| u16::from_le_bytes([image[i], image[i + 1]]) as usize;
    let table = word(0);
    if table == 0 && image.iter().all(|b| *b == 0) {
        return Ok(Vec::new());
    }
    if table == 0 || table % 4 != 0 || table > image.len() {
        return Err("宏目录格式未知，现有内容已保留".into());
    }
    let mut result = Vec::new();
    let mut previous_end = table;
    for i in (0..table).step_by(4) {
        let start = word(i);
        let end = start + word(i + 2);
        if start < previous_end || start >= end || end > IMAGE_LEN {
            return Err("宏目录超出已确认的两个设备页或包含重叠项，现有内容已保留".into());
        }
        result.push(start..end);
        previous_end = end;
    }
    Ok(result)
}

fn playback(raw: u8) -> Option<MacroMode> {
    match raw {
        1 => Some(MacroMode::Once),
        2 => Some(MacroMode::UntilOtherKey),
        4 => Some(MacroMode::WhileHeld),
        _ => None,
    }
}
fn mode_byte(mode: MacroMode) -> Result<u8> {
    match mode {
        MacroMode::Once => Ok(1),
        MacroMode::UntilOtherKey => Ok(2),
        MacroMode::WhileHeld => Ok(4),
        _ => Err("G75 不支持这种宏播放方式".into()),
    }
}
fn key_kind(usage: u16) -> Option<u8> {
    match usage {
        4..=99 => Some(0),
        101 | 224..=231 => Some(1),
        _ => None,
    }
}
fn decode_record(raw: &[u8], mode: MacroMode) -> Option<MacroDefinition> {
    let (&name_len, rest) = raw.split_first()?;
    let name = std::str::from_utf8(rest.get(..usize::from(name_len))?).ok()?;
    let data = &rest[name_len as usize..];
    if name.trim().is_empty() || data.is_empty() || data.len() % 4 != 0 {
        return None;
    }
    let mut events = Vec::new();
    for item in data.as_chunks::<4>().0 {
        let pressed = item[0] & 0x80 == 0;
        let usage = u16::from(item[3]);
        let kind = (item[0] >> 4) & 7;
        if item[0] & 8 != 0 {
            return None;
        }
        let event = if key_kind(usage) == Some(kind) {
            MacroEvent::Key { usage, pressed }
        } else if kind == 2 && [1, 2, 4, 8, 16].contains(&item[3]) {
            MacroEvent::Mouse {
                button: item[3].trailing_zeros() as u8 + 1,
                pressed,
            }
        } else {
            return None;
        };
        events.push(event);
        let mut delay = u32::from_be_bytes([0, item[0] & 7, item[1], item[2]]);
        while delay > 0 {
            let ms = delay.min(u16::MAX.into()) as u16;
            events.push(MacroEvent::Delay(ms));
            delay -= u32::from(ms);
        }
    }
    Some(MacroDefinition {
        name: name.into(),
        mode,
        events,
    })
}
fn encode_record(definition: &MacroDefinition) -> Result<Vec<u8>> {
    let name = definition.name.as_bytes();
    if definition.name.trim().is_empty() || name.len() > 255 {
        return Err("G75 宏名称须为 1 至 255 个 UTF-8 字节".into());
    }
    if definition.events.is_empty() || definition.events.len() > MAX_EVENTS {
        return Err(format!("G75 宏须为 1 至 {MAX_EVENTS} 步"));
    }
    let mut actions: Vec<[u8; 4]> = Vec::new();
    for event in &definition.events {
        let (kind, code, pressed) = match *event {
            MacroEvent::Key { usage, pressed } => (
                key_kind(usage).ok_or("G75 未定义此宏键码")?,
                usage as u8,
                pressed,
            ),
            MacroEvent::Mouse {
                button: b @ 1..=5,
                pressed,
            } => (2, 1 << (b - 1), pressed),
            MacroEvent::Delay(ms) => {
                let last = actions
                    .last_mut()
                    .ok_or("G75 的延迟须放在一个按键或鼠标动作之后")?;
                let delay = u32::from_be_bytes([0, last[0] & 7, last[1], last[2]]) + u32::from(ms);
                if delay > 0x7ffff || ms == 0 {
                    return Err("G75 每个动作后的总延迟须为 1 至 524287 毫秒".into());
                }
                let [_, high, middle, low] = delay.to_be_bytes();
                last[0] = (last[0] & !7) | high;
                last[1] = middle;
                last[2] = low;
                continue;
            }
            _ => return Err("G75 板载宏支持键盘、五个鼠标按钮和延迟".into()),
        };
        actions.push([kind << 4 | if pressed { 0 } else { 0x80 }, 0, 0, code]);
    }
    let mut raw = vec![name.len() as u8];
    raw.extend_from_slice(name);
    raw.extend(actions.into_iter().flatten());
    Ok(raw)
}

pub(super) fn populate(
    keyboard: &Keyboard,
    state: &mut ControlState,
    bindings: &[(u16, u8, u8)],
) -> Result<()> {
    let mut image = keyboard.read_macro_page(0)?;
    let directory_size = usize::from(u16::from_le_bytes([image[0], image[1]]));
    let records = if directory_size == 0 || directory_size > image.len() {
        image.extend(keyboard.read_macro_page(1)?);
        ranges(&image)?
    } else {
        let records = ranges(&image)?;
        if records.last().is_some_and(|r| r.end > image.len()) {
            image.extend(keyboard.read_macro_page(1)?);
        }
        records
    };
    state.macro_supported = true;
    state.macro_max_events = MAX_EVENTS;
    state.macro_name_max_bytes = Some(255);
    state.macro_modes = vec![
        MacroMode::Once,
        MacroMode::WhileHeld,
        MacroMode::UntilOtherKey,
    ];
    state.macro_event_types = ["key", "mouse", "delay"].map(String::from).to_vec();
    // MacroDefinition is indexed by the physical control in the shared API.
    // Only the base layer is editable in the macro editor.
    for &(control, mode, index) in bindings {
        if let Some(r) = records.get(usize::from(index)) {
            if let Some(definition) =
                playback(mode).and_then(|mode| decode_record(&image[r.clone()], mode))
            {
                state.macros.insert(control, definition);
            }
        }
    }
    Ok(())
}

// Keep existing directory indices even when several layers share a macro. Only
// a private record may be replaced. Unbound vendor records are still user data.
fn edit_image(
    original: &[u8],
    layers: &[Vec<u8>],
    control: usize,
    record: &[u8],
) -> Result<(Vec<u8>, u8)> {
    let old = ranges(original)?;
    let current = &layers[0][control * 4..control * 4 + 4];
    let mut records = old.iter().map(|r| &original[r.clone()]).collect::<Vec<_>>();
    let replace = if let [3, mode, 1, index] = *current {
        let r = old
            .get(usize::from(index))
            .ok_or("现有宏索引超出目录，已保留")?;
        if playback(mode)
            .and_then(|m| decode_record(&original[r.clone()], m))
            .is_none()
        {
            return Err("原宏含暂不能编辑的内容，已保留".into());
        }
        let refs = layers
            .iter()
            .flat_map(|l| l.as_chunks::<4>().0)
            .filter(|b| b[0] == 3 && b[3] == index)
            .count();
        (refs == 1).then_some(usize::from(index))
    } else {
        if current[0] == 3 {
            return Err("原键位宏格式尚未识别，已保留".into());
        }
        None
    };
    if let Some(index) = old.iter().position(|r| &original[r.clone()] == record) {
        return Ok((original.to_vec(), index as u8));
    }
    let slot = replace.unwrap_or(records.len());
    if replace.is_none()
        && layers
            .iter()
            .flat_map(|l| l.as_chunks::<4>().0)
            .any(|binding| binding[0] == 3 && usize::from(binding[3]) == slot)
    {
        return Err("新宏槽已被其他键位引用，现有绑定已保留".into());
    }
    if let Some(index) = replace {
        records[index] = record;
    } else {
        records.push(record);
    }
    let directory_len = records.len() * 4;
    let mut image = original.to_vec();
    image[..old.len() * 4].fill(0);
    for r in &old {
        image[r.clone()].fill(0);
    }
    // All bytes outside directory records can have unrelated content. A record
    // may move into a zero region, but never cover an opaque nonzero byte.
    let free: Vec<bool> = image.iter().map(|v| *v == 0).collect();
    if directory_len > image.len() || free[..directory_len].contains(&false) {
        return Err("设备宏目录没有可安全扩展的空间".into());
    }
    let mut next = directory_len;
    for (i, r) in records.iter().enumerate() {
        if r.len() > image.len() - next {
            return Err("G75 宏存储空间不足".into());
        }
        let last = image.len() - r.len();
        let allowed = |start: &usize| !free[*start..*start + r.len()].contains(&false);
        // The first record's offset *is* the directory size. Preserve old
        // positions whenever possible and keep subsequent record offsets sorted.
        let start = if i == 0 {
            allowed(&next).then_some(next)
        } else {
            old.get(i)
                .map(|o| o.start)
                .filter(|a| *a >= next && *a <= last && allowed(a))
                .or_else(|| (next..=last).find(allowed))
        }
        .ok_or("原有数据之间没有足够的空白空间，已保留原宏")?;
        next = start + r.len();
        image[i * 4..i * 4 + 2].copy_from_slice(&(start as u16).to_le_bytes());
        image[i * 4 + 2..i * 4 + 4].copy_from_slice(&(r.len() as u16).to_le_bytes());
        image[start..next].copy_from_slice(r);
    }
    Ok((image, slot as u8))
}
pub(super) fn apply(keyboard: &Keyboard, control: u16, definition: &MacroDefinition) -> Result<()> {
    let idx = usize::from(control);
    if !real_key(idx) || KEYS[idx] == "Fn" {
        return Err("G75 宏需要一个可以改键的普通键".into());
    }
    let mode = mode_byte(definition.mode)?;
    let record = encode_record(definition)?;
    let mut layers = Vec::new();
    for layer in 0..3 {
        let bytes = keyboard.read_block_layer(0x41, layer)?;
        validate_bindings(&bytes)?;
        layers.push(bytes);
    }
    let original = keyboard.read_macro_image()?;
    let (desired, slot) = edit_image(&original, &layers, idx, &record)?;
    let mut mapping = layers[0].clone();
    mapping[idx * 4..idx * 4 + 4].copy_from_slice(&[3, mode, 1, slot]);
    keyboard.write_macro_image(&desired, &original)?;
    if let Err(error) = keyboard.write_block_layer(1, 0x41, 0, &mapping, &layers[0]) {
        return match keyboard.write_macro_image(&original, &desired) {
            Ok(()) => Err(format!("{error}；已恢复原宏目录及内容")),
            Err(restore) => Err(format!("{error}；原宏恢复失败：{restore}")),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(name: &str) -> MacroDefinition {
        MacroDefinition {
            name: name.into(),
            mode: MacroMode::Once,
            events: vec![
                MacroEvent::Key {
                    usage: 225,
                    pressed: true,
                },
                MacroEvent::Delay(50),
                MacroEvent::Key {
                    usage: 225,
                    pressed: false,
                },
                MacroEvent::Mouse {
                    button: 4,
                    pressed: true,
                },
                MacroEvent::Delay(300),
                MacroEvent::Mouse {
                    button: 4,
                    pressed: false,
                },
            ],
        }
    }
    #[test]
    fn vendor_macro_event_bytes_names_modes_and_post_action_delays() {
        let definition = sample("宏");
        let expected = [
            3, 0xe5, 0xae, 0x8f, 0x10, 0, 50, 225, 0x90, 0, 0, 225, 0x20, 1, 44, 8, 0xa0, 0, 0, 8,
        ];
        assert_eq!(encode_record(&definition).unwrap(), expected);
        assert_eq!(
            decode_record(&expected, MacroMode::Once).unwrap().events,
            definition.events
        );
        for mode in [
            MacroMode::Once,
            MacroMode::UntilOtherKey,
            MacroMode::WhileHeld,
        ] {
            assert_eq!(playback(mode_byte(mode).unwrap()), Some(mode));
        }
        let mut bad = expected;
        bad[4] |= 8;
        assert!(decode_record(&bad, MacroMode::Once).is_none());
        let mut leading = sample("delay");
        leading.events.insert(0, MacroEvent::Delay(3));
        assert!(encode_record(&leading).is_err());
    }
    #[test]
    fn preserves_unbound_shared_opaque_records_and_unknown_padding_at_original_offsets() {
        let layers = vec![vec![0; 504]; 3];
        let data = encode_record(&sample("one")).unwrap();
        let (mut base, _) = edit_image(&vec![0; IMAGE_LEN], &layers, 9, &data).unwrap();
        let mut unknown = data.clone();
        unknown[4] |= 8;
        let (image, slot) = edit_image(&base, &layers, 9, &unknown).unwrap();
        assert_eq!(slot, 1);
        base = image;
        let end = ranges(&base).unwrap().last().unwrap().end;
        base[end..512].fill(0xa7);
        let mut bound = layers;
        for layer in &mut bound[..2] {
            layer[36..40].copy_from_slice(&[3, 1, 1, 0]);
        }
        let new = encode_record(&sample("new")).unwrap();
        let (edited, new_slot) = edit_image(&base, &bound, 9, &new).unwrap();
        let records = ranges(&edited).unwrap();
        assert_eq!(new_slot, 2);
        assert_eq!(&edited[records[0].clone()], data);
        assert_eq!(&edited[records[1].clone()], unknown);
        assert_eq!(&edited[records[2].clone()], new);
        assert_eq!(&edited[end..512], &base[end..512]);
        bound[0][36..40].copy_from_slice(&[3, 1, 1, 2]);
        let newer = encode_record(&sample("updated")).unwrap();
        let (updated, slot) = edit_image(&edited, &bound, 9, &newer).unwrap();
        assert_eq!(slot, 2);
        assert_eq!(ranges(&updated).unwrap().len(), 3);
        // A dangling reference in a hidden layer must not start triggering a
        // newly appended macro when the directory grows into that slot.
        bound[2][0..4].copy_from_slice(&[3, 1, 1, 3]);
        assert!(edit_image(
            &updated,
            &bound,
            10,
            &encode_record(&sample("four")).unwrap()
        )
        .is_err());
        let mut bad = updated;
        bad[0] = 3;
        assert!(ranges(&bad).is_err());
    }

    #[test]
    #[ignore = "Uses the attached G75 Pro, temporarily adds a macro and restores both pages and its key"]
    fn live_roundtrip_restores_original_pages_and_base_key() {
        fn run() -> Result<()> {
            let model = crate::devices::REGISTRY
                .iter()
                .position(|s| s.key == "mchose-g75-pro")
                .ok_or("model")?;
            let interface = crate::hid::enumerate()?
                .into_iter()
                .find(|i| crate::backend::matches(i, model))
                .ok_or("没有连接 G75 Pro")?;
            let keyboard = Keyboard::open(&interface.path)?;
            keyboard.verify_model()?;
            let original = keyboard.read_macro_image()?;
            let mapping = keyboard.read_block_layer(0x41, 0)?;
            validate_bindings(&mapping)?;
            let control = 72u16; // F12, only if its usual mapping is still present.
            if super::super::decode(&mapping[288..292])
                != (crate::controls::BindingAction::Keyboard {
                    usage: 69,
                    modifiers: 0,
                })
            {
                return Err("F12 已被重新定义，停止实机测试".into());
            }
            let dir =
                std::env::var_os("G75_TEST_BACKUP").ok_or("需要提供本次测试的全新备份路径")?;
            let dir = std::path::Path::new(&dir);
            std::fs::create_dir(dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("macro.bin"), &original).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("layer0.bin"), &mapping).map_err(|e| e.to_string())?;
            let definition = MacroDefinition {
                name: "PD-test".into(),
                mode: MacroMode::Once,
                events: vec![
                    MacroEvent::Key {
                        usage: 225,
                        pressed: true,
                    },
                    MacroEvent::Delay(15),
                    MacroEvent::Key {
                        usage: 225,
                        pressed: false,
                    },
                ],
            };
            let check = (|| -> Result<()> {
                apply(&keyboard, control, &definition)?;
                let state = super::super::read_sections(
                    &keyboard,
                    Some(crate::controls::ControlSection::Macros),
                )?;
                let read = state.macros.get(&control).ok_or("未读取测试宏")?;
                if read.name != definition.name
                    || read.mode != definition.mode
                    || read.events != definition.events
                {
                    return Err("实机测试宏定义与写入不符".into());
                }
                Ok(())
            })();
            // Restore from the actual current contents even after a partial failure.
            let restore_mapping = keyboard
                .read_block_layer(0x41, 0)
                .and_then(|current| keyboard.write_block_layer(1, 0x41, 0, &mapping, &current));
            let restore_image = keyboard
                .read_macro_image()
                .and_then(|current| keyboard.write_macro_image(&original, &current));
            restore_mapping?;
            restore_image?;
            if keyboard.read_macro_image()? != original
                || keyboard.read_block_layer(0x41, 0)? != mapping
            {
                return Err("实机测试还原后完整内容不符".into());
            }
            println!("G75 original two pages and base layer restored byte-for-byte.");
            check
        }
        run().unwrap();
    }
}
