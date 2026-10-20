use crate::{
    hid::{self, Device, Interface},
    protocol::{self, MchoseConfig, Result, MCHOSE_RATES},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const NAMES: [&str; 3] = ["炼狱蝰蛇 V3 专业版", "迈从 A5 Ultra", "迈从 G75 Pro"];

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub present: bool,
    pub fresh: bool,
    pub percent: Option<u8>,
    pub charging: Option<bool>,
    pub wired: bool,
    pub light: Option<u8>,
    pub dpi: Option<u16>,
    pub rate: Option<u16>,
    pub sleep_seconds: Option<u16>,
    pub updated: u64,
    pub identity: String,
    pub error: String,
    pub settings_error: String,
    pub alerted: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub devices: [State; 3],
    pub interval: u32,
    pub threshold: u8,
    pub notices: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            devices: [State::default(), State::default(), State::default()],
            interval: 60,
            threshold: 20,
            notices: true,
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
pub fn data_path() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("data")
        .join("preferences.json")
}
impl Snapshot {
    pub fn load() -> Self {
        let mut s: Self = std::fs::read(data_path())
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        s.interval = s.interval.clamp(30, 1800);
        s.threshold = s.threshold.clamp(5, 50);
        for d in &mut s.devices {
            if d.percent.is_some_and(|v| v > 100) {
                d.percent = None;
            }
            d.present = false;
            d.fresh = false;
            d.charging = None;
            d.dpi = None;
            d.rate = None;
            d.sleep_seconds = None;
        }
        s
    }
    pub fn save(&self) -> Result<()> {
        let p = data_path();
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = p.with_extension("tmp");
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        // Windows rename cannot replace an existing file. MoveFileExW performs
        // replacement atomically without a delete/write gap.
        unsafe {
            use windows_sys::Win32::Storage::FileSystem::*;
            if MoveFileExW(
                hid::wide(&temp.to_string_lossy()).as_ptr(),
                hid::wide(&p.to_string_lossy()).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        Ok(())
    }
    pub fn alerts(&mut self) -> Vec<usize> {
        let mut alerts = Vec::new();
        for (i, d) in self.devices.iter_mut().enumerate() {
            if !d.fresh {
                continue;
            }
            if let Some(p) = d.percent {
                if p >= self.threshold.saturating_add(5) {
                    d.alerted = false;
                }
                if self.notices && p < self.threshold && d.charging == Some(false) && !d.alerted {
                    alerts.push(i);
                    d.alerted = true;
                }
            }
        }
        alerts
    }
}

pub fn matches(i: &Interface, model: usize) -> bool {
    if model == 0 {
        i.vid == 0x1532
            && [0x00b6, 0x00b7].contains(&i.pid)
            && i.path.to_lowercase().contains("mi_00")
            && i.feature_len == 91
    } else if model == 1 {
        // Name and vendor AND descriptor shape are mandatory. A5 Pro/Max/V2,
        // G3 sharing a PID, and unrelated Realtek devices must never be queried.
        [0x5253, 0x0bda, 0x3837].contains(&i.vid)
            && ["a5 ultra", "mchose a5 ultra"].contains(&i.product.trim().to_lowercase().as_str())
            && i.usage_page == 0xff01
            && i.feature_len >= 65
    } else {
        ((i.vid == 0x3554 && i.pid == 0xfa09) || (i.vid == 0x41e4 && i.pid == 0x2001))
            && i.usage_page == 0xff02
            && i.input_len == 20
            && i.output_len == 20
    }
}

pub struct Channel {
    dev: Device,
    ready: Option<crate::keyboard::FeatureReady>,
    pub interface: Interface,
}
impl Channel {
    pub fn new(i: &Interface) -> Result<Self> {
        Ok(Self {
            dev: Device::open(&i.path)?,
            ready: if i.vid==0x1532 {None} else {Some(crate::keyboard::FeatureReady::open(&i.path)?)},
            interface: i.clone(),
        })
    }
    fn razer(&self, class: u8, command: u8, size: u8, args: &[u8]) -> Result<Vec<u8>> {
        let req = protocol::razer_request(class, command, size, args);
        self.dev.send(&req)?;
        let mut last = "等待设备回复超时".to_string();
        for _ in 0..6 {
            thread::sleep(Duration::from_millis(35));
            let reply = self.dev.receive(0, 91)?;
            match protocol::razer_reply(&reply, &req) {
                Ok(body) => return Ok(body.to_vec()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    fn mchose(&self, report: u8, command: u8, args: &[u8], read: bool) -> Result<Vec<u8>> {
        let req = protocol::mchose_request(report, command, args);
        let mut last = String::new();
        for _ in 0..if read {3} else {1} {
        self.dev.send(&req)?;
        if let Err(e)=self.ready.as_ref().unwrap().wait(command) {
            if !read {return Err(e);}
            last=e;continue;
        }
        if !read {return Ok(Vec::new());}
        for _ in 0..4 {
            let reply = self
                .dev
                .receive(report, self.interface.feature_len as usize)?;
            let len = if report == 0x11 { 21 } else { 65 };
            match protocol::mchose_reply(&reply[..len], report, command) {
                Ok(body) if command == 6 && body[2..4] == [0, 0] => {
                    last = "迈从鼠标暂未在线，保留上次电量".into()
                }
                Ok(body) if command == 0x67 && MchoseConfig::decode(&body).is_err() => {
                    last="迈从暂未返回有效配置，请关闭官方驱动后刷新".into()
                }
                Ok(body) => return Ok(body),
                Err(e) => last = e,
            }
            thread::sleep(Duration::from_millis(50));
        }
        thread::sleep(Duration::from_millis(150));
        }
        Err(last)
    }
    fn config(&self) -> Result<MchoseConfig> {
        MchoseConfig::decode(&self.mchose(0x12, 0x67, &[], true)?)
    }
    pub fn read(&self, model: usize) -> Result<State> {
        let mut s = State {
            present: true,
            fresh: true,
            updated: now(),
            identity: format!("{:04X}:{:04X}", self.interface.vid, self.interface.pid),
            ..State::default()
        };
        if model == 0 {
            s.percent = Some(protocol::razer_percent(self.razer(7, 0x80, 2, &[])?[1]));
            s.charging = self.razer(7, 0x84, 2, &[]).ok().and_then(|v| match v[1] {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            });
            s.wired = self.interface.pid == 0xb6;
            // Each optional setting fails independently of the battery reading.
            match self.razer(4, 0x85, 7, &[1]) {
                Ok(v) => s.dpi = Some(u16::from_be_bytes([v[1], v[2]])),
                Err(e) => s.settings_error.push_str(&format!("DPI: {e}; ")),
            }
            match self.razer(0, 0x85, 1, &[]) {
                Ok(v) if [1, 2, 8].contains(&v[0]) => s.rate = Some(1000 / v[0] as u16),
                Ok(_) => s.settings_error.push_str("回报率字段未知; "),
                Err(e) => s.settings_error.push_str(&format!("回报率: {e}; ")),
            }
            match self.razer(7, 0x83, 2, &[]) {
                Ok(v) => s.sleep_seconds = Some(u16::from_be_bytes([v[0], v[1]])),
                Err(e) => s.settings_error.push_str(&format!("休眠: {e}")),
            }
        } else {
            let (p, charging, wired) =
                protocol::mchose_battery(&self.mchose(0x11, 6, &[], true)?)?;
            s.percent = Some(p);
            s.charging = charging;
            s.wired = wired;
            match self.config() {
                Ok(c) => {
                    s.dpi = Some(c.dpi(wired));
                    s.rate = Some(MCHOSE_RATES[if wired { c.wired_rate } else { c.wireless_rate }]);
                    s.sleep_seconds = Some(c.raw[19] as u16 * 60);
                }
                Err(e) => s.settings_error = e,
            }
        }
        Ok(s)
    }
    pub fn apply(&self, model: usize, action: Action) -> Result<()> {
        let current = self.read(model)?;
        match (model, action) {
            (0, Action::Dpi(dpi)) if (100..=30000).contains(&dpi) && dpi % 50 == 0 => {
                let [hi, lo] = dpi.to_be_bytes();
                self.razer(4, 5, 7, &[1, hi, lo, hi, lo, 0, 0])?;
            }
            (0, Action::Rate(rate)) if [125, 500, 1000].contains(&rate) => {
                self.razer(0, 5, 1, &[(1000 / rate) as u8])?;
            }
            (0, Action::Sleep(seconds)) if (60..=900).contains(&seconds) => {
                self.razer(7, 3, 2, &seconds.to_be_bytes())?;
            }
            (1, Action::Dpi(dpi)) if (50..=26000).contains(&dpi) && dpi % 50 == 0 => {
                let cfg = self.config()?;
                self.mchose(0x12, 0x40, &cfg.dpi_arguments(dpi, current.wired), false)?;
            }
            (1, Action::Rate(rate))
                if MCHOSE_RATES.contains(&rate) && (current.wired || rate <= 4000) =>
            {
                let cfg = self.config()?;
                let code = MCHOSE_RATES.iter().position(|r| *r == rate).unwrap() as u8;
                let (usb, wireless) = if current.wired {
                    (code, cfg.wireless_rate as u8)
                } else {
                    (cfg.wired_rate as u8, code)
                };
                self.mchose(0x11, 0x41, &[usb, wireless], false)?;
            }
            (1, Action::Sleep(seconds)) if (60..=3600).contains(&seconds) && seconds % 60 == 0 => {
                self.mchose(0x11, 0x0a, &[1, (seconds / 60) as u8], false)?;
            }
            _ => return Err("参数不在该设备的已知范围内".into()),
        }
        thread::sleep(Duration::from_millis(180));
        let check = self.read(model)?;
        let (actual, expected) = match action {
            Action::Dpi(v) => (check.dpi, v),
            Action::Rate(v) => (check.rate, v),
            Action::Sleep(v) => (check.sleep_seconds, v),
            Action::Light(_) => unreachable!(),
        };
        if actual != Some(expected) {
            return Err(format!(
                "已发送，但读回未确认：期望 {expected}，实际 {actual:?}。请关闭官方驱动后刷新"
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Dpi(u16),
    Rate(u16),
    Sleep(u16),
    Light(u8),
}

pub fn refresh(previous: &Snapshot, action: Option<(usize, Action)>) -> (Snapshot, Result<()>) {
    let mut next = previous.clone();
    let interfaces = match hid::enumerate() {
        Ok(v) => v,
        Err(e) => {
            for d in &mut next.devices {
                d.fresh = false;
                d.charging = None;
                d.error = e.clone();
            }
            return (next, Err(e));
        }
    };
    let mut action_result = Ok(());
    for model in 0..3 {
        let mut candidates: Vec<_> = interfaces.iter().filter(|i| matches(i, model)).collect();
        candidates.sort_by_key(|i| if model == 0 && i.pid == 0xb6 { 0 } else { 1 });
        let d = &mut next.devices[model];
        d.present = !candidates.is_empty();
        d.fresh = false;
        d.charging = None;
        d.dpi = None;
        d.rate = None;
        d.sleep_seconds = None;
        if candidates.is_empty() {
            d.error = "未检测到支持的接口，请连接接收器或 USB 数据线".into();
            if action.is_some_and(|a| a.0 == model) {
                action_result = Err(d.error.clone());
            }
            continue;
        }
        let mut read = None;
        for interface in candidates {
            let result = if model == 2 {
                crate::keyboard::Keyboard::open(&interface.path).and_then(|k| {
                    if let Some((2, a)) = action {
                        action_result = k.apply(a);
                    }
                    k.read(format!("{:04X}:{:04X}", interface.vid, interface.pid))
                })
            } else {
                Channel::new(interface).and_then(|channel| {
                    if let Some((m, a)) = action {
                        if m == model {
                            action_result = channel.apply(model, a);
                        }
                    }
                    channel.read(model)
                })
            };
            match result {
                Ok(mut s) => {
                    s.alerted = d.alerted;
                    read = Some(s);
                    break;
                }
                Err(e) => d.error = e,
            }
        }
        if let Some(s) = read {
            *d = s;
        } else if action.is_some_and(|a| a.0==model) {
            action_result=Err(d.error.clone());
        }
    }
    (next, action_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires all three physical peripherals; writes the existing values and checks read-back"]
    fn live_settings_readback() {
        let all=hid::enumerate().unwrap();
        let mut results=Vec::new();
        for (model,name) in NAMES.iter().enumerate() {
            let i=all.iter().find(|i|matches(i,model)).expect("Required peripheral missing");
            if model<2 {
                let channel=Channel::new(i).unwrap();
                let before=channel.read(model).unwrap();
                let actions=[Action::Dpi(before.dpi.unwrap()),Action::Rate(before.rate.unwrap()),Action::Sleep(before.sleep_seconds.unwrap())];
                for action in actions {
                    thread::sleep(Duration::from_millis(450));
                    let result=channel.apply(model,action);
                    let line=format!("{name} {action:?}: {result:?}");println!("{line}");results.push(line);
                }
            } else {
                let k=crate::keyboard::Keyboard::open(&i.path).unwrap();
                let before=k.read(format!("{:04X}:{:04X}",i.vid,i.pid)).unwrap();
                let actions=[Action::Rate(before.rate.unwrap()),Action::Sleep(before.sleep_seconds.unwrap()),Action::Light(before.light.unwrap())];
                for action in actions {
                    thread::sleep(Duration::from_millis(450));
                    let result=k.apply(action);
                    let line=format!("{name} {action:?}: {result:?}");println!("{line}");results.push(line);
                }
            }
        }
        for result in &results {println!("{result}");}
        assert!(results.iter().all(|r|r.ends_with("Ok(())")),"Hardware read-back failed");
    }
    #[test]
    fn notification_cycle_ignores_stale_and_charging() {
        let mut s = Snapshot::default();
        s.devices[0] = State {
            fresh: true,
            percent: Some(10),
            charging: Some(true),
            ..State::default()
        };
        assert!(s.alerts().is_empty());
        s.devices[0].charging = Some(false);
        assert_eq!(s.alerts(), vec![0]);
        assert!(s.alerts().is_empty());
        s.devices[0].percent = Some(25);
        s.alerts();
        s.devices[0].percent = Some(10);
        s.devices[0].fresh = false;
        assert!(s.alerts().is_empty());
        s.devices[0].fresh = true;
        assert_eq!(s.alerts(), vec![0]);
    }
    #[test]
    fn a5_name_guard_excludes_other_generations_and_shared_pids() {
        let mut i = Interface {
            path: String::new(),
            vid: 0x0bda,
            pid: 0x4762,
            usage_page: 0xff01,
            usage: 1,
            feature_len: 65,
            input_len: 0,
            output_len: 0,
            product: "A5 ultra".into(),
        };
        assert!(matches(&i, 1));
        for name in [
            "MCHOSE G3 Ultra 4K",
            "MCHOSE A5 V2 Ultra",
            "A5 Pro Max",
            "USB Receiver",
        ] {
            i.product = name.into();
            assert!(!matches(&i, 1));
        }
    }
}
