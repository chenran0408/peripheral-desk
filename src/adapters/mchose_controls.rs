//! First-generation A5 Ultra's M HUB button and single-page onboard macros.
//! Source: official Gst/Fst/nJ schemas, not the unrelated A5 Pro Max protocol.
use super::mouse::Channel;
use crate::{
    controls::{
        Binding, BindingAction, ControlAction, ControlState, MacroDefinition, MacroEvent, MacroMode,
    },
    protocol::Result,
};
use std::collections::{BTreeMap, BTreeSet};

const BUTTONS: [&str; 6] = ["左键", "滚轮中键", "右键", "前进侧键", "后退侧键", "DPI 键"];
const OFFSET: usize = 20;
const MAX_EVENTS: usize = 13;

fn index(control: u16) -> Result<usize> {
    if control < 6 {
        Ok(control as usize)
    } else {
        Err("A5 Ultra 只支持六个已知按键".into())
    }
}
fn entry(raw: &[u8], button: usize) -> [u8; 4] {
    raw[OFFSET + button * 4..OFFSET + button * 4 + 4]
        .try_into()
        .unwrap()
}
fn action(button: usize, bytes: [u8; 4]) -> BindingAction {
    let value = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]);
    match (bytes[0] & 15, value) {
        (0, 0) => match button {
            0 => BindingAction::Mouse(1),
            1 => BindingAction::Mouse(3),
            2 => BindingAction::Mouse(2),
            3 => BindingAction::Mouse(5),
            4 => BindingAction::Mouse(4),
            _ => BindingAction::DpiCycle,
        },
        (1, 0x010000) => BindingAction::Mouse(1),
        (1, 0x020000) => BindingAction::Mouse(2),
        (1, 0x040000) => BindingAction::Mouse(3),
        (1, 0x080000) => BindingAction::Mouse(4),
        (1, 0x100000) => BindingAction::Mouse(5),
        (2, _) if bytes[3] == 0 && (4..=0xe7).contains(&bytes[2]) => BindingAction::Keyboard {
            usage: bytes[2] as u16,
            modifiers: bytes[1],
        },
        (3, _) if bytes[2..] == [0, 0] => BindingAction::Consumer(bytes[1] as u16),
        (4, _) => BindingAction::Macro(button as u16),
        (5, 0x010000) => BindingAction::DpiCycle,
        (9, 0xffffff) => BindingAction::Disabled,
        _ => BindingAction::Unknown(bytes.to_vec()),
    }
}
fn encode(button: usize, action: &BindingAction) -> Result<[u8; 4]> {
    let (kind, value) = match *action {
        BindingAction::Disabled => (9, 0xffffff),
        BindingAction::Mouse(v @ 1..=5) => (
            1,
            [0x010000, 0x020000, 0x040000, 0x080000, 0x100000][v as usize - 1],
        ),
        BindingAction::Keyboard { usage, modifiers } if (4..=0xe7).contains(&usage) => {
            (2, ((modifiers as u32) << 16) | ((usage as u32) << 8))
        }
        BindingAction::Consumer(value)
            if [0xcd, 0xb5, 0xb6, 0xb7, 0xe9, 0xea, 0xe2].contains(&value) =>
        {
            (3, (value as u32) << 16)
        }
        BindingAction::DpiCycle => (5, 0x010000),
        BindingAction::Macro(_) => {
            return Err("请通过宏编辑器写入 A5 板载宏，不能引用未确认的宏槽".into())
        }
        _ => return Err("A5 Ultra 尚不支持该改键动作".into()),
    };
    Ok([
        ((button as u8) << 4) | kind,
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ])
}
fn check_table(raw: &[u8]) -> Result<()> {
    if raw.len() < 63 || (0..6).any(|b| entry(raw, b)[0] >> 4 != b as u8) {
        return Err("A5 按键表格式未识别，停止写入".into());
    }
    Ok(())
}
pub fn read(channel: &Channel) -> Result<ControlState> {
    let config = channel.config()?;
    check_table(&config.raw)?;
    let mut macros = BTreeMap::new();
    let mut notice = "A5 板载宏支持单次、最多 13 步键盘/延迟；仅已确认空页或已核对的本工具宏可写，未知非空页保持只读。改键须保留至少一个左键。".to_string();
    let library = macro_library()?;
    for (&control, definition) in &library {
        let button = index(control)?;
        if entry(&config.raw, button) != macro_entry(button) {
            continue;
        }
        match verify_macro(channel, button, definition) {
            Ok(()) => {
                macros.insert(control, definition.clone());
            }
            Err(error) => notice.push_str(&format!(
                " {}的本地宏未匹配设备，保持只读：{error}。",
                BUTTONS[button]
            )),
        }
    }
    // Empty-looking length-zero replies are not a confirmed macro backup.
    // Advertise writing only if at least one non-left button passes the same
    // page, ownership and binding checks used by apply(); never probe by writing.
    let mut macro_supported = false;
    for button in 1..6 {
        if matches!(
            action(button, entry(&config.raw, button)),
            BindingAction::Mouse(1) | BindingAction::Unknown(_)
        ) || (entry(&config.raw, button)[0] & 15 == 4
            && entry(&config.raw, button) != macro_entry(button))
            || ensure_left(&config.raw, button, macro_entry(button)).is_err()
        {
            continue;
        }
        if snapshot(channel, button)
            .and_then(|page| permitted_page(&page, button, library.get(&(button as u16))))
            .is_ok()
        {
            macro_supported = true;
            break;
        }
    }
    if !macro_supported {
        notice.push_str(" 当前没有已确认可安全恢复的非左键宏页，宏写入已关闭：宏页格式或来源尚未确认；已保存且核对一致的宏仍可只读查看。");
    }
    Ok(ControlState {
        binding_types: ["mouse", "keyboard", "consumer", "disabled", "dpi-cycle"]
            .map(String::from)
            .to_vec(),
        bindings: (0..6)
            .map(|button| {
                let action = action(button, entry(&config.raw, button));
                Binding {
                    control: button as u16,
                    layer: 0,
                    label: BUTTONS[button].into(),
                    editable: !matches!(action, BindingAction::Unknown(_)),
                    action,
                }
            })
            .collect(),
        macro_supported,
        macro_max_events: MAX_EVENTS,
        macros,
        notice,
        ..ControlState::default()
    })
}
fn write_entry(channel: &Channel, button: usize, value: [u8; 4]) -> Result<()> {
    channel.mchose(
        0x12,
        0x52,
        &[button as u8, 0, value[0] & 15, value[1], value[2], value[3]],
        false,
    )?;
    Ok(())
}
fn require_config(channel: &Channel, expected: &[u8]) -> Result<()> {
    if channel.config()?.raw != expected {
        return Err("A5 完整配置读回与预期不符".into());
    }
    Ok(())
}
fn restore(channel: &Channel, button: usize, original: &[u8], error: String) -> Result<()> {
    match write_entry(channel, button, entry(original, button))
        .and_then(|_| require_config(channel, original))
    {
        Ok(()) => Err(format!("{error}；已恢复原按键功能")),
        Err(e) => Err(format!("{error}；原按键恢复失败：{e}")),
    }
}
fn ensure_left(raw: &[u8], button: usize, replacement: [u8; 4]) -> Result<()> {
    if !(0..6).any(|b| {
        action(
            b,
            if b == button {
                replacement
            } else {
                entry(raw, b)
            },
        ) == BindingAction::Mouse(1)
    }) {
        return Err("请至少保留一个左键功能，再修改当前按键".into());
    }
    Ok(())
}
fn macro_arguments(button: usize, definition: &MacroDefinition) -> Result<Vec<u8>> {
    if definition.mode != MacroMode::Once {
        return Err("A5 首批板载宏只开放单次执行".into());
    }
    if definition.events.is_empty() || definition.events.len() > MAX_EVENTS {
        return Err("A5 板载宏需要 1–13 个事件".into());
    }
    if definition.name.trim().is_empty()
        || definition.name.len() > 20
        || definition.name.contains('\0')
    {
        return Err("宏名称需要 1–20 个 UTF-8 字节且不能含空字符".into());
    }
    // Single-page Gst: trigger=1, condition=1 (Once), time=0; 58-byte
    // data area consists of six metadata bytes and up to 52 event bytes.
    let mut args = vec![
        button as u8,
        0,
        0,
        0,
        58,
        1,
        1,
        0,
        0,
        0,
        definition.events.len() as u8,
    ];
    let mut held = BTreeSet::new();
    for event in &definition.events {
        match *event {
            MacroEvent::Key { usage, pressed } if (4..=0xe7).contains(&usage) => {
                if pressed {
                    if !held.insert(usage) {
                        return Err("宏中包含重复按下且未释放的按键".into());
                    }
                } else if !held.remove(&usage) {
                    return Err("宏中包含没有对应按下的释放事件".into());
                }
                args.extend_from_slice(&[0, if pressed { 0x81 } else { 1 }, usage as u8, 0]);
            }
            MacroEvent::Delay(ms) if ms > 0 => {
                args.extend_from_slice(&[0, 15, ms as u8, (ms >> 8) as u8])
            }
            _ => return Err("A5 首批板载宏仅支持键盘按下/释放与 1–65535ms 延迟".into()),
        }
    }
    if !held.is_empty() {
        return Err("单次宏结束前必须释放所有按键".into());
    }
    Ok(args)
}
fn macro_path() -> std::path::PathBuf {
    crate::backend::data_path().with_file_name("a5-macros.json")
}
fn macro_library() -> Result<BTreeMap<u16, MacroDefinition>> {
    use std::io::Read;
    let file = match std::fs::File::open(macro_path()) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("读取本地 A5 宏记录失败：{e}")),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("本地 A5 宏记录过大".into());
    }
    let definitions: BTreeMap<u16, MacroDefinition> =
        serde_json::from_slice(&bytes).map_err(|e| format!("本地 A5 宏记录格式错误：{e}"))?;
    for (control, definition) in &definitions {
        macro_arguments(index(*control)?, definition)?;
    }
    Ok(definitions)
}
fn macro_entry(button: usize) -> [u8; 4] {
    [(button as u8) << 4 | 4, 0, 0, 0]
}
fn name_arguments(button: usize, definition: &MacroDefinition) -> Vec<u8> {
    let mut name = vec![button as u8, definition.name.len() as u8];
    name.extend_from_slice(definition.name.as_bytes());
    name
}
fn verify_macro(channel: &Channel, button: usize, definition: &MacroDefinition) -> Result<()> {
    let args = macro_arguments(button, definition)?;
    let read = channel.mchose(0x12, 0x65, &[button as u8, 0, 0, 0, 0], true)?;
    // Compare the complete single-page header and exactly count*4 active
    // event bytes. Padding past count is not part of a macro definition.
    if read.len() < args.len() || read[..args.len()] != args {
        return Err("A5 板载宏事件读回不一致".into());
    }
    let name = name_arguments(button, definition);
    let read = channel.mchose(0x12, 0x63, &[button as u8], true)?;
    if read.len() < name.len() || read[..name.len()] != name {
        return Err("A5 宏名称读回不一致".into());
    }
    Ok(())
}
#[derive(Clone, Debug)]
struct MacroSnapshot {
    page: Vec<u8>,
    name: Vec<u8>,
}
fn parse_snapshot(button: usize, page: Vec<u8>, name: &[u8]) -> Result<MacroSnapshot> {
    if page.len() != 63 || page[..5] != [button as u8, 0, 0, 0, 58] || page[10] > MAX_EVENTS as u8 {
        return Err("原宏页不是已确认可完整恢复的单页格式，已拒绝覆盖".into());
    }
    if name.len() < 2
        || name[0] != button as u8
        || name[1] as usize + 2 > name.len()
        || name[1] as usize + 2 > 63
    {
        return Err("原宏名称长度或编号未确认，已拒绝覆盖".into());
    }
    Ok(MacroSnapshot {
        page,
        name: name[..2 + name[1] as usize].to_vec(),
    })
}
fn snapshot(channel: &Channel, button: usize) -> Result<MacroSnapshot> {
    let page = channel.mchose(0x12, 0x65, &[button as u8, 0, 0, 0, 0], true)?;
    let name = channel.mchose(0x12, 0x63, &[button as u8], true)?;
    parse_snapshot(button, page, &name)
}
fn matches_definition(
    original: &MacroSnapshot,
    button: usize,
    definition: &MacroDefinition,
) -> Result<bool> {
    let expected = macro_arguments(button, definition)?;
    Ok(original.page[..expected.len()] == expected
        && original.name == name_arguments(button, definition))
}
fn confirmed_empty(original: &MacroSnapshot) -> bool {
    original.page[5..].iter().all(|byte| *byte == 0) && original.name[1] == 0
}
fn permitted_page(
    original: &MacroSnapshot,
    button: usize,
    definition: Option<&MacroDefinition>,
) -> Result<()> {
    if confirmed_empty(original)
        || definition.is_some_and(|definition| {
            matches_definition(original, button, definition).unwrap_or(false)
        })
    {
        Ok(())
    } else {
        Err(
            "该按键含未匹配本工具记录的非空宏页，无法安全覆盖；只支持已确认空页或已核对的本工具宏"
                .into(),
        )
    }
}
fn macro_page(
    original: &MacroSnapshot,
    button: usize,
    definition: &MacroDefinition,
) -> Result<Vec<u8>> {
    let mut desired = original.page.clone();
    let encoded = macro_arguments(button, definition)?;
    desired[..encoded.len()].copy_from_slice(&encoded);
    Ok(desired)
}
fn require_snapshot(channel: &Channel, button: usize, expected: &MacroSnapshot) -> Result<()> {
    let actual = snapshot(channel, button)?;
    if actual.page != expected.page || actual.name != expected.name {
        return Err("原宏页/名称完整读回未匹配".into());
    }
    Ok(())
}
fn write_macro(
    channel: &Channel,
    button: usize,
    definition: &MacroDefinition,
    original: &MacroSnapshot,
) -> Result<()> {
    let desired = MacroSnapshot {
        page: macro_page(original, button, definition)?,
        name: name_arguments(button, definition),
    };
    channel.mchose(0x12, 0x55, &desired.page, false)?;
    channel.mchose(0x12, 0x53, &desired.name, false)?;
    require_snapshot(channel, button, &desired)
}
fn restore_macro(
    channel: &Channel,
    button: usize,
    original: &MacroSnapshot,
    config: &[u8],
    error: String,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = channel.mchose(0x12, 0x55, &original.page, false) {
        failures.push(error);
    }
    if let Err(error) = channel.mchose(0x12, 0x53, &original.name, false) {
        failures.push(error);
    }
    if let Err(error) = write_entry(channel, button, entry(config, button)) {
        failures.push(error);
    }
    if let Err(error) = require_snapshot(channel, button, original) {
        failures.push(error);
    }
    if let Err(error) = require_config(channel, config) {
        failures.push(error);
    }
    if failures.is_empty() {
        Err(format!("{error}；已恢复原始完整宏页、名称和按键配置"))
    } else {
        Err(format!(
            "{error}；原宏恢复未能完整确认：{}",
            failures.join("；")
        ))
    }
}
pub fn apply(channel: &Channel, request: &ControlAction) -> Result<()> {
    let config = channel.config()?;
    check_table(&config.raw)?;
    match request {
        ControlAction::Bind {
            control,
            layer,
            action: desired,
        } => {
            let button = index(*control)?;
            if *layer != 0 {
                return Err("A5 Ultra 没有已确认的第二改键层".into());
            }
            if action(button, entry(&config.raw, button)) == *desired {
                return Ok(());
            }
            let encoded = encode(button, desired)?;
            ensure_left(&config.raw, button, encoded)?;
            let mut expected = config.raw.clone();
            expected[OFFSET + button * 4..OFFSET + button * 4 + 4].copy_from_slice(&encoded);
            if let Err(error) = write_entry(channel, button, encoded)
                .and_then(|_| require_config(channel, &expected))
            {
                return restore(channel, button, &config.raw, error);
            }
            Ok(())
        }
        ControlAction::Macro {
            control,
            definition,
        } => {
            let button = index(*control)?;
            macro_arguments(button, definition)?;
            let mut library = macro_library()?;
            if matches!(
                action(button, entry(&config.raw, button)),
                BindingAction::Unknown(_)
            ) {
                return Err("原按键配置未知，已拒绝覆盖".into());
            }
            if entry(&config.raw, button)[0] & 15 == 4
                && entry(&config.raw, button) != macro_entry(button)
            {
                return Err("原宏绑定格式未确认，已拒绝覆盖".into());
            }
            let original_macro = snapshot(channel, button)?;
            permitted_page(&original_macro, button, library.get(control))?;
            let encoded = macro_entry(button);
            ensure_left(&config.raw, button, encoded)?;
            let outcome = (|| {
                write_macro(channel, button, definition, &original_macro)?;
                let mut expected = config.raw.clone();
                expected[OFFSET + button * 4..OFFSET + button * 4 + 4].copy_from_slice(&encoded);
                require_config(channel, &expected)?;
                library.insert(*control, definition.clone());
                crate::storage::write_json_preserving(
                    &macro_path(),
                    serde_json::to_value(&library).map_err(|e| e.to_string())?,
                )
            })();
            if let Err(error) = outcome {
                return restore_macro(channel, button, &original_macro, &config.raw, error);
            }
            Ok(())
        }
        ControlAction::Colors(_) => Err("A5 Ultra 没有已确认的逐键灯效协议".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn macro_page_gate_rejects_unknown_content_and_keeps_unused_bytes() {
        let mut page = vec![0; 63];
        page[..5].copy_from_slice(&[3, 0, 0, 0, 58]);
        let empty = parse_snapshot(3, page.clone(), &[3, 0]).unwrap();
        assert!(permitted_page(&empty, 3, None).is_ok());
        page[62] = 0xa5;
        let unknown = parse_snapshot(3, page.clone(), &[3, 0]).unwrap();
        assert!(permitted_page(&unknown, 3, None).is_err());
        let definition = MacroDefinition {
            name: "A".into(),
            mode: MacroMode::Once,
            events: vec![
                MacroEvent::Key {
                    usage: 4,
                    pressed: true,
                },
                MacroEvent::Key {
                    usage: 4,
                    pressed: false,
                },
            ],
        };
        let arguments = macro_arguments(3, &definition).unwrap();
        page[..arguments.len()].copy_from_slice(&arguments);
        let owned = parse_snapshot(3, page, &name_arguments(3, &definition)).unwrap();
        assert!(permitted_page(&owned, 3, Some(&definition)).is_ok());
        assert_eq!(macro_page(&owned, 3, &definition).unwrap()[62], 0xa5);
        let mut multipage = owned.page.clone();
        multipage[1] = 1;
        assert!(parse_snapshot(3, multipage, &owned.name).is_err());
    }
    #[test]
    fn a5_controls_match_official_standalone_and_macro_schemas() {
        assert_eq!(
            encode(
                3,
                &BindingAction::Keyboard {
                    usage: 0x42,
                    modifiers: 0
                }
            )
            .unwrap(),
            [0x32, 0, 0x42, 0]
        );
        let definition = MacroDefinition {
            name: "A".into(),
            mode: MacroMode::Once,
            events: vec![
                MacroEvent::Key {
                    usage: 4,
                    pressed: true,
                },
                MacroEvent::Delay(50),
                MacroEvent::Key {
                    usage: 4,
                    pressed: false,
                },
            ],
        };
        assert_eq!(
            macro_arguments(3, &definition).unwrap(),
            [3, 0, 0, 0, 58, 1, 1, 0, 0, 0, 3, 0, 0x81, 4, 0, 0, 15, 50, 0, 0, 1, 4, 0]
        );
        let mut invalid = definition;
        invalid.events.pop();
        assert!(macro_arguments(3, &invalid).is_err());
    }
}
