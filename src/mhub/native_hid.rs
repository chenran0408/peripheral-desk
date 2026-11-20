//! Native HID configuration collections for the local M HUB document.
mod allowlist;
mod descriptor;
mod io;

use descriptor::{Collection, Device};
use io::Transfer;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{Arc, Mutex},
    thread,
};
use windows_sys::Win32::{
    Devices::HumanInterfaceDevice::*, Foundation::*, Storage::FileSystem::FILE_FLAG_OVERLAPPED,
    System::Threading::*,
};

type Emit = Arc<dyn Fn(Value) + Send + Sync>;
struct Endpoint {
    handle: Arc<OwnedHandle>,
    caps: Collection,
}
struct Lease(String);
impl Drop for Lease {
    fn drop(&mut self) {
        crate::plugin_manager::release_web_driver_instance(&self.0);
    }
}

struct Session {
    _lease: Lease,
    stop: Arc<OwnedHandle>,
    endpoints: Vec<Endpoint>,
}
impl Session {
    fn cancel(&self) {
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
    }
    fn open(device: &Device) -> Result<Self, String> {
        for spec in crate::devices::REGISTRY.iter().filter(|s| {
            (s.supports("software-macro-trigger")
                || matches!(s.key, "mchose-a5-ultra" | "mchose-g75-pro"))
                && device.collections.iter().any(|c| s.matches(&c.interface))
        }) {
            let macros = crate::resident_macros::list(spec.key)?;
            if macros.library.recovery.iter().any(|r| {
                r.target_instance
                    .as_deref()
                    .is_none_or(|id| id == device.id)
            }) {
                return Err(format!(
                    "{} 正在使用常驻宏，请先在偏好设置中恢复该设备的原键。",
                    spec.name
                ));
            }
        }
        crate::plugin_manager::claim_web_driver_instance(&device.id)?;
        let lease = Lease(device.id.clone());
        let mut endpoints = Vec::new();
        for c in &device.collections {
            let mut access = 0;
            if c.interface.input_len > 0 {
                access |= GENERIC_READ;
            }
            if c.interface.output_len > 0 {
                access |= GENERIC_WRITE;
            }
            let handle = io::open_handle(&c.interface.path, access, FILE_FLAG_OVERLAPPED)
                .map_err(|e| e.to_string())?;
            if c.interface.input_len > 0 {
                // Large keyboard responses span more than the default 32 input reports.
                if !unsafe { HidD_SetNumInputBuffers(handle.as_raw_handle(), 128) } {
                    return Err(std::io::Error::last_os_error().to_string());
                }
            }
            endpoints.push(Endpoint {
                handle: Arc::new(handle),
                caps: c.clone(),
            });
        }
        Ok(Self {
            _lease: lease,
            stop: Arc::new(io::event().map_err(|e| e.to_string())?),
            endpoints,
        })
    }
    fn operation(&self, op: &str, id: u8, data: Option<&Value>) -> Result<Value, String> {
        let ty = if op == "sendOutput" {
            HidP_Output
        } else {
            HidP_Feature
        };
        let ep = self
            .endpoints
            .iter()
            .find(|e| e.caps.reports[ty as usize].contains_key(&id))
            .ok_or("设备没有声明该类型的报告 ID")?;
        let size = if ty == HidP_Output {
            ep.caps.interface.output_len
        } else {
            ep.caps.interface.feature_len
        } as usize;
        let mut wire = vec![id];
        if op == "receiveFeature" {
            wire.resize(size, 0);
            let n = io::transfer(&ep.handle, &self.stop, Transfer::GetFeature, &mut wire)
                .map_err(|e| e.to_string())?;
            // Windows excludes an unnumbered report's zero placeholder from its byte count.
            let prefix = usize::from(wire[0] == 0);
            let data = wire
                .get(prefix..)
                .and_then(|bytes| bytes.get(..n))
                .filter(|bytes| !bytes.is_empty())
                .ok_or("设备返回的功能报告长度异常")?;
            return Ok(json!(data));
        }
        let payload = data.and_then(Value::as_array).ok_or("缺少报告数据")?;
        if payload.len() + 1 > size {
            return Err("报告超出该集合允许的长度".into());
        }
        for b in payload {
            wire.push(
                b.as_u64()
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or("报告数据必须为字节")?,
            );
        }
        if ty == HidP_Output {
            wire.resize(size, 0);
            let n = io::transfer(&ep.handle, &self.stop, Transfer::Output, &mut wire)
                .map_err(|e| e.to_string())?;
            if n != wire.len() {
                return Err("输出报告未完整发送".into());
            }
        } else {
            io::transfer(&ep.handle, &self.stop, Transfer::SetFeature, &mut wire)
                .map_err(|e| e.to_string())?;
        }
        Ok(Value::Null)
    }
}

#[derive(Default)]
struct State {
    target: Option<String>,
    stopped: bool,
    scanned: bool,
    devices: BTreeMap<String, Device>,
    sessions: BTreeMap<String, Arc<Session>>,
}
pub(super) struct Bridge {
    state: Arc<Mutex<State>>,
    emit: Emit,
}
impl Bridge {
    pub(super) fn new(emit: impl Fn(Value) + Send + Sync + 'static) -> Self {
        Self {
            state: Default::default(),
            emit: Arc::new(emit),
        }
    }
    pub(super) fn set_target(&self, target: Option<String>) {
        let mut s = self.state.lock().unwrap();
        if s.target != target {
            s.target = target;
            s.scanned = false;
        }
    }
    pub(super) fn shutdown(&self) {
        let mut s = self.state.lock().unwrap();
        s.stopped = true;
        for (_, session) in std::mem::take(&mut s.sessions) {
            session.cancel();
        }
        s.devices.clear();
    }
    pub(super) fn refresh(&self) -> Result<(), String> {
        let mut devices = descriptor::enumerate()?;
        let events = {
            let mut s = self.state.lock().unwrap();
            if s.stopped {
                return Err("本地设备会话已关闭".into());
            }
            if let Some(wanted) = &s.target {
                devices.retain(|id, _| id == wanted);
            }
            let mut events = Vec::new();
            if s.scanned {
                for id in s
                    .devices
                    .keys()
                    .filter(|id| !devices.contains_key(*id))
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    if let Some(session) = s.sessions.remove(&id) {
                        session.cancel();
                    }
                    events.push(json!({"op":"disconnect","device":id}));
                }
                for (id, d) in &devices {
                    if !s.devices.contains_key(id) {
                        events.push(json!({"op":"connect","descriptor":d.descriptor()}));
                    }
                }
            }
            s.devices = devices;
            s.scanned = true;
            events
        };
        for event in events {
            (self.emit)(event);
        }
        Ok(())
    }
    pub(super) fn request(&self, r: &Value) -> Result<Value, String> {
        let op = r["op"].as_str().ok_or("缺少 HID 操作")?;
        if op == "list" {
            if !self.state.lock().unwrap().scanned {
                self.refresh()?;
            }
            let s = self.state.lock().unwrap();
            if s.stopped {
                return Err("本地设备会话已关闭".into());
            }
            return Ok(Value::Array(
                s.devices.values().map(Device::descriptor).collect(),
            ));
        }
        let id = r["device"].as_str().ok_or("缺少设备标识")?;
        let mut s = self.state.lock().unwrap();
        if s.stopped {
            return Err("本地设备会话已关闭".into());
        }
        if op == "close" {
            if let Some(session) = s.sessions.remove(id) {
                session.cancel();
            }
            return Ok(Value::Null);
        }
        let device = s.devices.get(id).ok_or("此设备已断开或不支持本地连接")?;
        if op == "open" {
            if s.sessions.contains_key(id) {
                return Ok(Value::Null);
            }
            let device = device.clone();
            drop(s);
            let session = Arc::new(Session::open(&device)?);
            let mut s = self.state.lock().unwrap();
            if s.stopped || !s.devices.contains_key(id) {
                return Err("设备会话已结束".into());
            }
            if s.sessions.contains_key(id) {
                return Ok(Value::Null);
            }
            s.sessions.insert(id.into(), session.clone());
            self.listen(id, &session);
            return Ok(Value::Null);
        }
        if !matches!(op, "sendFeature" | "receiveFeature" | "sendOutput") {
            return Err("未知 HID 操作".into());
        }
        let report = r["reportId"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok())
            .ok_or("报告 ID 无效")?;
        let session = s.sessions.get(id).cloned().ok_or("设备尚未打开")?;
        drop(s);
        session.operation(op, report, r.get("data"))
    }
    fn listen(&self, id: &str, session: &Arc<Session>) {
        for ep in &session.endpoints {
            let size = ep.caps.interface.input_len as usize;
            if size == 0 {
                continue;
            }
            let handle = ep.handle.clone();
            let stop = session.stop.clone();
            let state = self.state.clone();
            let id = id.to_string();
            let emit = self.emit.clone();
            thread::spawn(move || {
                let mut wire = vec![0; size];
                loop {
                    match io::transfer(&handle, &stop, Transfer::Input, &mut wire) {
                        Ok(n) if n > 0 && n <= wire.len() => {
                            let s = state.lock().unwrap();
                            if !s
                                .sessions
                                .get(&id)
                                .is_some_and(|current| Arc::ptr_eq(&current.stop, &stop))
                            {
                                return;
                            }
                            emit(
                                json!({"op":"input","device":id,"reportId":wire[0],"data":&wire[1..n]}),
                            );
                        }
                        _ => {
                            let mut s = state.lock().unwrap();
                            if s.sessions
                                .get(&id)
                                .is_some_and(|current| Arc::ptr_eq(&current.stop, &stop))
                            {
                                s.sessions.remove(&id).unwrap().cancel();
                                s.devices.remove(&id);
                                drop(s);
                                emit(json!({"op":"disconnect","device":id}));
                            }
                            return;
                        }
                    }
                }
            });
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests;
