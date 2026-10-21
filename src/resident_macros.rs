//! Desktop-only software macros. Device firmware stores only a reversible trigger binding.
use crate::{
    controls::{self, BindingAction, ControlAction, MacroEvent},
    devices::REGISTRY,
    protocol::Result,
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
        Arc, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub id: u64,
    pub control: u16,
    pub layer: u8,
    pub name: String,
    pub events: Vec<MacroEvent>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recovery {
    pub id: u64,
    pub control: u16,
    pub layer: u8,
    pub trigger_usage: u16,
    pub original: BindingAction,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Library {
    pub schema: u32,
    pub device: String,
    #[serde(default)]
    pub definitions: Vec<Definition>,
    #[serde(default)]
    pub recovery: Vec<Recovery>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub library: Library,
    pub active: Vec<u64>,
    pub notice: String,
}
fn model(key: &str) -> Result<usize> {
    REGISTRY
        .iter()
        .position(|s| s.key == key)
        .filter(|index| {
            REGISTRY[*index].supports("software-macro-trigger")
                || matches!(
                    key,
                    "razer-deathadder-v3-pro" | "mchose-a5-ultra" | "mchose-g75-pro"
                )
        })
        .ok_or_else(|| "此设备尚未提供可恢复的软件宏触发改键".into())
}
fn path(key: &str) -> Result<PathBuf> {
    model(key)?;
    Ok(crate::backend::data_path()
        .parent()
        .unwrap()
        .join("devices")
        .join(key)
        .join("resident-macros.json"))
}
fn validate(library: &Library, key: &str) -> Result<()> {
    if library.schema != 1
        || library.device != key
        || library.definitions.len() > 64
        || library.recovery.len() > 12
    {
        return Err("软件宏配置版本、设备或数量不匹配".into());
    }
    let mut ids = BTreeSet::new();
    for definition in &library.definitions {
        if definition.id == 0
            || !ids.insert(definition.id)
            || definition.layer > 2
            || definition.name.trim().is_empty()
            || definition.name.chars().count() > 32
            || definition.name.chars().any(char::is_control)
        {
            return Err("软件宏名称、编号或层无效".into());
        }
        validate_events(&definition.events)?;
    }
    let mut slots = BTreeSet::new();
    let mut triggers = BTreeSet::new();
    let mut recoveries = BTreeSet::new();
    for recovery in &library.recovery {
        if recovery.id == 0
            || !recoveries.insert(recovery.id)
            || !slots.insert((recovery.control, recovery.layer))
            || !triggers.insert(recovery.trigger_usage)
            || !(104..=115).contains(&recovery.trigger_usage)
            || recovery.layer > 2
            || matches!(
                recovery.original,
                BindingAction::Unknown(_) | BindingAction::Macro(_)
            )
        {
            return Err("软件宏恢复记录无效，已保留原文件".into());
        }
    }
    Ok(())
}
fn load(key: &str) -> Result<Library> {
    let path = path(key)?;
    let library = match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > 1024 * 1024 {
                return Err("软件宏配置过大".into());
            }
            serde_json::from_slice(&bytes).map_err(|e| format!("软件宏配置不可读取：{e}"))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Library {
            schema: 1,
            device: key.into(),
            definitions: vec![],
            recovery: vec![],
            extra: BTreeMap::new(),
        },
        Err(e) => return Err(e.to_string()),
    };
    validate(&library, key)?;
    Ok(library)
}
fn persist(library: &Library) -> Result<()> {
    validate(library, &library.device)?;
    storage::atomic_write(
        &path(&library.device)?,
        &serde_json::to_vec_pretty(library).map_err(|e| e.to_string())?,
    )
}
/// Import only dormant definitions. Local recovery records must never be replaced by another machine's.
pub fn import_value(key: &str, incoming: Value, previous: Option<Value>) -> Result<Value> {
    model(key)?;
    let mut next = previous.unwrap_or_else(
        || serde_json::json!({"schema":1,"device":key,"definitions":[],"recovery":[]}),
    );
    let recovery = next
        .get("recovery")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let mut incoming = incoming;
    if !incoming.is_object() {
        return Err("软件宏配置应为对象".into());
    }
    incoming["recovery"] = recovery;
    let definitions = incoming.get("definitions").cloned();
    storage::merge_json(&mut next, incoming);
    // Explicit removal must not resurrect records through keyed-array merging.
    if let Some(definitions) = definitions {
        next["definitions"] = definitions;
    }
    let library: Library = serde_json::from_value(next.clone()).map_err(|e| e.to_string())?;
    validate(&library, key)?;
    Ok(next)
}
pub fn export_value(mut value: Value) -> Value {
    if let Some(fields) = value.as_object_mut() {
        fields.remove("recovery");
    }
    value
}

fn usage_vk(usage: u16) -> Result<u16> {
    Ok(match usage {
        4..=29 => 0x41 + usage - 4,
        30..=38 => 0x31 + usage - 30,
        39 => 0x30,
        40 => VK_RETURN,
        41 => VK_ESCAPE,
        42 => VK_BACK,
        43 => VK_TAB,
        44 => VK_SPACE,
        45 => VK_OEM_MINUS,
        46 => VK_OEM_PLUS,
        47 => VK_OEM_4,
        48 => VK_OEM_6,
        49 => VK_OEM_5,
        51 => VK_OEM_1,
        52 => VK_OEM_7,
        53 => VK_OEM_3,
        54 => VK_OEM_COMMA,
        55 => VK_OEM_PERIOD,
        56 => VK_OEM_2,
        57 => VK_CAPITAL,
        58..=69 => VK_F1 + usage - 58,
        70 => VK_SNAPSHOT,
        71 => VK_SCROLL,
        72 => VK_PAUSE,
        73 => VK_INSERT,
        74 => VK_HOME,
        75 => VK_PRIOR,
        76 => VK_DELETE,
        77 => VK_END,
        78 => VK_NEXT,
        79 => VK_RIGHT,
        80 => VK_LEFT,
        81 => VK_DOWN,
        82 => VK_UP,
        83 => VK_NUMLOCK,
        84 => VK_DIVIDE,
        85 => VK_MULTIPLY,
        86 => VK_SUBTRACT,
        87 => VK_ADD,
        89..=97 => VK_NUMPAD1 + usage - 89,
        98 => VK_NUMPAD0,
        99 => VK_DECIMAL,
        101 => VK_APPS,
        224 => VK_LCONTROL,
        225 => VK_LSHIFT,
        226 => VK_LMENU,
        227 => VK_LWIN,
        228 => VK_RCONTROL,
        229 => VK_RSHIFT,
        230 => VK_RMENU,
        231 => VK_RWIN,
        _ => return Err("软件宏只支持已映射的普通键/修饰键；F13–F24 保留作触发，不可播放".into()),
    })
}
pub fn validate_events(events: &[MacroEvent]) -> Result<()> {
    if events.is_empty() || events.len() > 128 {
        return Err("软件宏需要 1–128 步".into());
    }
    let mut held = BTreeSet::new();
    let mut elapsed = 0u32;
    for event in events {
        match *event {
            MacroEvent::Key { usage, pressed } => {
                usage_vk(usage)?;
                if pressed {
                    if !held.insert(usage) {
                        return Err("宏中重复按下尚未释放的键".into());
                    }
                } else if !held.remove(&usage) {
                    return Err("宏中释放了尚未按下的键".into());
                }
            }
            MacroEvent::Delay(ms) if ms > 0 => {
                elapsed += u32::from(ms);
                if elapsed > 30000 {
                    return Err("软件宏总延时不能超过 30 秒".into());
                }
            }
            _ => return Err("软件宏仅支持键盘按下、释放和正延时".into()),
        }
    }
    if !held.is_empty() {
        return Err("宏结束前必须释放全部按键".into());
    }
    Ok(())
}
fn trigger(usage: u16) -> BindingAction {
    BindingAction::Keyboard {
        usage,
        modifiers: 0,
    }
}
fn binding(key: &str, control: u16, layer: u8) -> Result<BindingAction> {
    controls::read(model(key)?)?
        .bindings
        .into_iter()
        .find(|b| b.control == control && b.layer == layer)
        .map(|b| b.action)
        .ok_or_else(|| "未读到待恢复的原按键".into())
}
fn write_binding(key: &str, control: u16, layer: u8, action: BindingAction) -> Result<()> {
    let state = controls::apply(
        model(key)?,
        &ControlAction::Bind {
            control,
            layer,
            action: action.clone(),
        },
    )?;
    if !state
        .bindings
        .iter()
        .any(|b| b.control == control && b.layer == layer && b.action == action)
    {
        return Err("软件宏触发绑定未通过读回确认".into());
    }
    Ok(())
}
struct Active {
    key: String,
    id: u64,
    usage: u16,
    events: Vec<MacroEvent>,
}
struct Playback {
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<()>>,
}
struct Runtime {
    active: BTreeMap<i32, Active>,
    playback: Option<Playback>,
    notice: String,
}
impl Runtime {
    fn stop_playback(&mut self) {
        if let Some(playback) = self.playback.take() {
            playback.cancel.store(true, Ordering::Release);
            match playback.handle.join() {
                Ok(Err(e)) => self.notice = e,
                Err(_) => self.notice = "宏播放意外结束，已尝试释放按键".into(),
                _ => {}
            }
        }
    }
    fn snapshot(&self, key: &str) -> Result<Snapshot> {
        Ok(Snapshot {
            library: load(key)?,
            active: self
                .active
                .values()
                .filter(|a| a.key == key)
                .map(|a| a.id)
                .collect(),
            notice: self.notice.clone(),
        })
    }
    fn enable(&mut self, key: &str, id: u64) -> Result<()> {
        let mut library = load(key)?;
        if self.active.values().any(|a| a.key == key && a.id == id) {
            return Err("该软件宏已在常驻运行".into());
        }
        if library
            .recovery
            .iter()
            .any(|r| !self.active.values().any(|a| a.key == key && a.id == r.id))
        {
            return Err("该设备还有待恢复绑定，请先点击停用/恢复".into());
        }
        let definition = library
            .definitions
            .iter()
            .find(|d| d.id == id)
            .cloned()
            .ok_or("未找到软件宏")?;
        if library
            .recovery
            .iter()
            .any(|r| r.control == definition.control && r.layer == definition.layer)
        {
            return Err("该按键已有活动宏，先停用再编辑".into());
        }
        let state = controls::read(model(key)?)?;
        let selected = state
            .bindings
            .iter()
            .find(|b| b.control == definition.control && b.layer == definition.layer)
            .filter(|b| b.editable)
            .ok_or("当前按键不可改写或未读到有效绑定")?;
        if (key == "mchose-a5-ultra" && definition.control == 0)
            || (key == "razer-deathadder-v3-pro" && definition.control == 1)
            || matches!(
                selected.action,
                BindingAction::Mouse(1) | BindingAction::Unknown(_) | BindingAction::Macro(_)
            )
        {
            return Err("左键、保留 Fn、未知配置和板载宏不能作为软件宏触发键".into());
        }
        let mut registration = None;
        for usage in 104..=115u16 {
            if self.active.values().any(|a| a.usage == usage)
                || state
                    .bindings
                    .iter()
                    .any(|b| matches!(b.action,BindingAction::Keyboard{usage:u,..}if u==usage))
            {
                continue;
            }
            let hotkey = 5000 + i32::from(usage);
            if unsafe {
                RegisterHotKey(
                    null_mut(),
                    hotkey,
                    MOD_NOREPEAT,
                    u32::from(VK_F13 + usage - 104),
                )
            } != 0
            {
                registration = Some((hotkey, usage));
                break;
            }
        }
        let (hotkey, usage) = registration.ok_or("F13–F24 触发键均被占用，未改写设备")?;
        let recovery = Recovery {
            id,
            control: definition.control,
            layer: definition.layer,
            trigger_usage: usage,
            original: selected.action.clone(),
            extra: BTreeMap::new(),
        };
        library.recovery.push(recovery.clone());
        if let Err(e) = persist(&library) {
            unsafe { UnregisterHotKey(null_mut(), hotkey) };
            return Err(format!("无法先保存恢复记录，未改写设备：{e}"));
        }
        let result = write_binding(key, definition.control, definition.layer, trigger(usage));
        if let Err(error) = result {
            let restore = self.restore(key, &recovery);
            unsafe { UnregisterHotKey(null_mut(), hotkey) };
            return Err(format!(
                "启用软件宏失败：{error}；{}",
                match restore {
                    Ok(()) => "已核对恢复原绑定".into(),
                    Err(e) => format!("恢复未确认，记录已保留：{e}"),
                }
            ));
        }
        self.active.insert(
            hotkey,
            Active {
                key: key.into(),
                id,
                usage,
                events: definition.events,
            },
        );
        self.notice = "软件宏已启用，需保持程序常驻；退出或停用时恢复原绑定".into();
        Ok(())
    }
    fn restore(&mut self, key: &str, recovery: &Recovery) -> Result<()> {
        let current = binding(key, recovery.control, recovery.layer)?;
        if current != recovery.original {
            if current != trigger(recovery.trigger_usage) {
                return Err("当前绑定已被其他设置修改，未覆盖该新值；原绑定恢复记录仍保留".into());
            }
            write_binding(
                key,
                recovery.control,
                recovery.layer,
                recovery.original.clone(),
            )?;
        }
        let mut library = load(key)?;
        library.recovery.retain(|r| r.id != recovery.id);
        persist(&library)
    }
    fn disable(&mut self, key: &str, id: u64) -> Result<()> {
        self.stop_playback();
        let recovery = load(key)?.recovery.into_iter().find(|r| r.id == id);
        let result = if let Some(recovery) = recovery {
            self.restore(key, &recovery)
        } else {
            Ok(())
        };
        let ids: Vec<_> = self
            .active
            .iter()
            .filter(|(_, a)| a.key == key && a.id == id)
            .map(|(h, _)| *h)
            .collect();
        for hotkey in ids {
            unsafe { UnregisterHotKey(null_mut(), hotkey) };
            self.active.remove(&hotkey);
        }
        result
    }
    fn disable_device(&mut self, key: &str) -> Result<()> {
        let library = load(key)?;
        let mut errors = Vec::new();
        for recovery in library.recovery {
            if let Err(e) = self.disable(key, recovery.id) {
                errors.push(format!("宏 {}：{e}", recovery.id))
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "软件宏原绑定尚未全部恢复，恢复记录已保留：{}",
                errors.join("；")
            ))
        }
    }
    fn poll(&mut self) {
        if self
            .playback
            .as_ref()
            .is_some_and(|p| p.handle.is_finished())
        {
            if let Some(playback) = self.playback.take() {
                self.notice = match playback.handle.join() {
                    Ok(Ok(())) => "软件宏已执行一次".into(),
                    Ok(Err(e)) => e,
                    Err(_) => "播放线程异常，已尝试释放按键".into(),
                }
            }
        }
        unsafe {
            let mut message: MSG = zeroed();
            while PeekMessageW(&mut message, null_mut(), WM_HOTKEY, WM_HOTKEY, PM_REMOVE) != 0 {
                if self.playback.is_some() {
                    continue;
                }
                if let Some(active) = self.active.get(&(message.wParam as i32)) {
                    let events = active.events.clone();
                    let cancel = Arc::new(AtomicBool::new(false));
                    let cancelled = cancel.clone();
                    match thread::Builder::new()
                        .name("peripheral-macro-playback".into())
                        .spawn(move || play(&events, cancelled))
                    {
                        Ok(handle) => self.playback = Some(Playback { cancel, handle }),
                        Err(e) => self.notice = format!("无法启动软件宏播放：{e}"),
                    }
                }
            }
        }
    }
}
enum Operation {
    List(String),
    Save(String, Option<u64>, u16, u8, String, Vec<MacroEvent>),
    Delete(String, u64),
    Enable(String, u64),
    Disable(String, u64),
    Device(String),
    Shutdown,
}
enum Reply {
    State(Snapshot),
    Id(u64),
    Done,
}
struct Request {
    operation: Operation,
    reply: Sender<Result<Reply>>,
}
struct Manager {
    sender: Sender<Request>,
}
static MANAGER: OnceLock<Result<Manager>> = OnceLock::new();
fn manager() -> Result<&'static Manager> {
    MANAGER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Request>();
            thread::Builder::new()
                .name("peripheral-resident-macros".into())
                .spawn(move || {
                    unsafe {
                        let mut message: MSG = zeroed();
                        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                    }
                    let mut runtime = Runtime {
                        active: BTreeMap::new(),
                        playback: None,
                        notice: String::new(),
                    };
                    loop {
                        runtime.poll();
                        let request = match receiver.recv_timeout(Duration::from_millis(20)) {
                            Ok(r) => r,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        };
                        let shutdown = matches!(request.operation, Operation::Shutdown);
                        let result = (|| -> Result<Reply> {
                            match request.operation {
                                Operation::List(key) => Ok(Reply::State(runtime.snapshot(&key)?)),
                                Operation::Save(key, id, control, layer, name, events) => {
                                    let mut library = load(&key)?;
                                    validate_events(&events)?;
                                    if library.recovery.iter().any(|r| {
                                        Some(r.id) == id
                                            || (r.control == control && r.layer == layer)
                                    }) {
                                        return Err("先停用并恢复该按键，再编辑软件宏".into());
                                    }
                                    let id = match id {
                                        Some(id) => id,
                                        None => {
                                            let mut id = (SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_nanos()
                                                as u64)
                                                .max(1);
                                            while library.definitions.iter().any(|d| d.id == id) {
                                                id = id.checked_add(1).ok_or("软件宏编号已用尽")?;
                                            }
                                            id
                                        }
                                    };
                                    if let Some(record) =
                                        library.definitions.iter_mut().find(|d| d.id == id)
                                    {
                                        record.control = control;
                                        record.layer = layer;
                                        record.name = name;
                                        record.events = events;
                                    } else {
                                        library.definitions.push(Definition {
                                            id,
                                            control,
                                            layer,
                                            name,
                                            events,
                                            extra: BTreeMap::new(),
                                        });
                                    }
                                    persist(&library)?;
                                    Ok(Reply::Id(id))
                                }
                                Operation::Delete(key, id) => {
                                    let mut library = load(&key)?;
                                    if library.recovery.iter().any(|r| r.id == id) {
                                        return Err("先恢复原按键，再删除软件宏".into());
                                    }
                                    library.definitions.retain(|d| d.id != id);
                                    persist(&library)?;
                                    Ok(Reply::Done)
                                }
                                Operation::Enable(key, id) => {
                                    runtime.enable(&key, id)?;
                                    Ok(Reply::Done)
                                }
                                Operation::Disable(key, id) => {
                                    runtime.disable(&key, id)?;
                                    Ok(Reply::Done)
                                }
                                Operation::Device(key) => {
                                    runtime.disable_device(&key)?;
                                    Ok(Reply::Done)
                                }
                                Operation::Shutdown => {
                                    runtime.stop_playback();
                                    let keys: BTreeSet<_> =
                                        runtime.active.values().map(|a| a.key.clone()).collect();
                                    let mut errors = Vec::new();
                                    for key in keys {
                                        if let Err(e) = runtime.disable_device(&key) {
                                            errors.push(e)
                                        }
                                    }
                                    // Even failed recovery must release every OS registration before exit.
                                    for (hotkey, _) in std::mem::take(&mut runtime.active) {
                                        unsafe { UnregisterHotKey(null_mut(), hotkey) };
                                    }
                                    if errors.is_empty() {
                                        Ok(Reply::Done)
                                    } else {
                                        Err(errors.join("；"))
                                    }
                                }
                            }
                        })();
                        let _ = request.reply.send(result);
                        if shutdown {
                            break;
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(Manager { sender })
        })
        .as_ref()
        .map_err(Clone::clone)
}
fn request(operation: Operation) -> Result<Reply> {
    let (reply, receiver) = mpsc::channel();
    manager()?
        .sender
        .send(Request { operation, reply })
        .map_err(|_| "软件宏引擎已关闭")?;
    receiver.recv().map_err(|_| "软件宏引擎意外结束")?
}
pub fn list(key: &str) -> Result<Snapshot> {
    match request(Operation::List(key.into()))? {
        Reply::State(state) => Ok(state),
        _ => Err("软件宏返回类型异常".into()),
    }
}
pub fn save(
    key: &str,
    id: Option<u64>,
    control: u16,
    layer: u8,
    name: &str,
    events: Vec<MacroEvent>,
) -> Result<u64> {
    match request(Operation::Save(
        key.into(),
        id,
        control,
        layer,
        name.trim().into(),
        events,
    ))? {
        Reply::Id(id) => Ok(id),
        _ => Err("软件宏返回类型异常".into()),
    }
}
pub fn delete(key: &str, id: u64) -> Result<()> {
    request(Operation::Delete(key.into(), id)).map(|_| ())
}
pub fn enable(key: &str, id: u64) -> Result<()> {
    request(Operation::Enable(key.into(), id)).map(|_| ())
}
pub fn disable(key: &str, id: u64) -> Result<()> {
    request(Operation::Disable(key.into(), id)).map(|_| ())
}
pub fn disable_device(key: &str) -> Result<()> {
    if model(key).is_err() {
        return Ok(());
    }
    request(Operation::Device(key.into())).map(|_| ())
}
pub fn shutdown() -> Result<()> {
    if MANAGER.get().is_none() {
        return Ok(());
    }
    request(Operation::Shutdown).map(|_| ())
}

fn send_key(usage: u16, pressed: bool) -> Result<()> {
    let vk = usage_vk(usage)?;
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if pressed { 0 } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0x50444d43,
            },
        },
    };
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        return Err("系统未接受宏键盘输入；请检查目标窗口权限，已尝试释放按键".into());
    }
    Ok(())
}
struct Held(BTreeSet<u16>);
impl Drop for Held {
    fn drop(&mut self) {
        for usage in &self.0 {
            let _ = send_key(*usage, false);
        }
    }
}
fn play(events: &[MacroEvent], cancel: Arc<AtomicBool>) -> Result<()> {
    validate_events(events)?;
    // Do not release a key that the user was already physically holding.
    for event in events {
        if let MacroEvent::Key { usage, .. } = event {
            if unsafe { GetAsyncKeyState(i32::from(usage_vk(*usage)?)) } & (0x8000u16 as i16) != 0 {
                return Err("宏涉及的按键正被按住，本次未播放".into());
            }
        }
    }
    let mut held = Held(BTreeSet::new());
    let deadline = Instant::now() + Duration::from_secs(30);
    for event in events {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err("软件宏已停止，已尝试释放按键".into());
        }
        match *event {
            MacroEvent::Key { usage, pressed } => {
                if pressed {
                    held.0.insert(usage);
                }
                send_key(usage, pressed)?;
                if !pressed {
                    held.0.remove(&usage);
                }
            }
            MacroEvent::Delay(ms) => {
                let end = (Instant::now() + Duration::from_millis(u64::from(ms))).min(deadline);
                while Instant::now() < end {
                    if cancel.load(Ordering::Acquire) {
                        return Err("软件宏已停止，已尝试释放按键".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            _ => unreachable!("validated above"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn software_events_reject_recursion_unbalanced_keys_and_excess_time() {
        let key = |usage, pressed| MacroEvent::Key { usage, pressed };
        assert!(validate_events(&[key(4, true), MacroEvent::Delay(20), key(4, false)]).is_ok());
        assert!(validate_events(&[key(104, true), key(104, false)]).is_err());
        assert!(validate_events(&[key(4, true)]).is_err());
        assert!(validate_events(&[MacroEvent::Delay(30000), MacroEvent::Delay(1)]).is_err());
        assert!(validate_events(&vec![MacroEvent::Delay(1); 129]).is_err());
    }
    #[test]
    fn imported_macros_cannot_replace_local_recovery_or_auto_enable() {
        let key = "mchose-a5-ultra";
        let recovery = serde_json::json!({"id":8,"control":3,"layer":0,"trigger_usage":104,"original":{"Mouse":5},"future":7});
        let original = serde_json::json!({"schema":1,"device":key,"definitions":[],"recovery":[recovery],"future":9});
        let incoming = serde_json::json!({"schema":1,"device":key,"definitions":[],"recovery":[],"enabled":true});
        let imported = import_value(key, incoming, Some(original)).unwrap();
        assert_eq!(imported["recovery"][0]["id"], 8);
        assert_eq!(imported["future"], 9);
        assert!(export_value(imported).get("recovery").is_none());
    }
}
