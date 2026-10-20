//! Razer and M HUB mouse sessions. Wire codecs live in protocol.rs.
use crate::{
    backend::now,
    devices::Adapter,
    hid::{Device, Interface},
    model::{Action, State, Tuning},
    protocol::{self, MchoseConfig, Result, MCHOSE_RATES},
};
use std::{thread, time::Duration};
pub struct Channel {
    dev: Device,
    ready: Option<crate::keyboard::FeatureReady>,
    pub interface: Interface,
    pub spec: &'static crate::devices::DeviceSpec,
}
impl Channel {
    pub fn new(i: &Interface, spec: &'static crate::devices::DeviceSpec) -> Result<Self> {
        Ok(Self {
            spec,
            dev: Device::open(&i.path)?,
            ready: if i.vid == 0x1532 {
                None
            } else {
                Some(crate::keyboard::FeatureReady::open(&i.path)?)
            },
            interface: i.clone(),
        })
    }
    fn razer(&self, class: u8, command: u8, size: u8, args: &[u8]) -> Result<Vec<u8>> {
        let req = protocol::razer_request(class, command, size, args);
        let mut last = "等待设备回复超时".to_string();
        // Receiver firmware can remain busy when settings are queried in a burst.
        // Reissue only read commands; never duplicate a setting write.
        for _ in 0..if command & 0x80 != 0 { 2 } else { 1 } {
            self.dev.send(&req)?;
            for poll in 0..6 {
                thread::sleep(Duration::from_millis(if poll == 0 { 80 } else { 35 }));
                let reply = self.dev.receive(0, 91)?;
                match protocol::razer_reply(&reply, &req) {
                    Ok(body) => return Ok(body.to_vec()),
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }
    fn mchose(&self, report: u8, command: u8, args: &[u8], read: bool) -> Result<Vec<u8>> {
        let req = protocol::mchose_request(report, command, args);
        let mut last = String::new();
        for _ in 0..if read { 3 } else { 1 } {
            self.dev.send(&req)?;
            if let Err(e) = self.ready.as_ref().unwrap().wait(command) {
                if !read {
                    return Err(e);
                }
                last = e;
                continue;
            }
            if !read {
                return Ok(Vec::new());
            }
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
                        last = "迈从暂未返回有效配置，请关闭官方驱动后刷新".into()
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
    pub fn read(&self) -> Result<State> {
        let mut s = State {
            present: true,
            fresh: true,
            updated: now(),
            identity: format!("{:04X}:{:04X}", self.interface.vid, self.interface.pid),
            ..State::default()
        };
        if self.spec.adapter == Adapter::Razer {
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
            if let Ok(v) = self.razer(7, 0x81, 2, &[]) {
                s.tuning
                    .insert(Tuning::LowPower, protocol::razer_percent(v[0]) as u16);
            }
            if let Ok(v) = self.razer(4, 0x86, 38, &[0]) {
                if let Ok((stages, active)) = protocol::razer_stage_table(&v) {
                    s.dpi_stages = stages;
                    s.active_stage = Some(active);
                }
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
                    s.dpi_stages = (0..c.stages)
                        .map(|i| u16::from_le_bytes([c.raw[4 + i * 2], c.raw[5 + i * 2]]))
                        .collect();
                    s.active_stage = Some(if wired {
                        c.wired_stage
                    } else {
                        c.wireless_stage
                    } as u8);
                    s.tuning.insert(Tuning::Debounce, c.raw[18] as u16);
                    s.tuning.insert(Tuning::Lod, (c.raw[17] & 3) as u16);
                    for (key, mask) in [
                        (Tuning::Ripple, 4),
                        (Tuning::Angle, 8),
                        (Tuning::Motion, 16),
                    ] {
                        s.tuning.insert(key, u16::from(c.raw[17] & mask != 0));
                    }
                }
                Err(e) => s.settings_error = e,
            }
        }
        Ok(s)
    }
    pub fn apply(&self, action: Action) -> Result<()> {
        let current = self.read()?;
        match (self.spec.adapter, action) {
            (Adapter::Razer, Action::Dpi(dpi)) if (100..=30000).contains(&dpi) && dpi % 50 == 0 => {
                if let Some(stage) = current.active_stage {
                    let table = self.razer(4, 0x86, 38, &[0])?;
                    let args = protocol::razer_stage_arguments(&table, stage, Some(dpi))?;
                    self.razer(4, 6, 38, &args)?;
                }
                let [hi, lo] = dpi.to_be_bytes();
                self.razer(4, 5, 7, &[1, hi, lo, hi, lo, 0, 0])?;
            }
            (Adapter::Razer, Action::Tune(Tuning::LowPower, v)) if (5..=50).contains(&v) => {
                let raw = ((v * 255 + 50) / 100) as u8;
                self.razer(7, 1, 2, &[raw, 0])?;
            }
            (Adapter::Razer, Action::Stage(stage)) => {
                let table = self.razer(4, 0x86, 38, &[0])?;
                let args = protocol::razer_stage_arguments(&table, stage, None)?;
                self.razer(4, 6, 38, &args)?;
                let at = 3 + stage as usize * 7;
                self.razer(
                    4,
                    5,
                    7,
                    &[
                        1,
                        args[at + 1],
                        args[at + 2],
                        args[at + 3],
                        args[at + 4],
                        0,
                        0,
                    ],
                )?;
            }
            (Adapter::Razer, Action::Rate(rate)) if [125, 500, 1000].contains(&rate) => {
                self.razer(0, 5, 1, &[(1000 / rate) as u8])?;
            }
            (Adapter::Razer, Action::Sleep(seconds)) if (60..=900).contains(&seconds) => {
                self.razer(7, 3, 2, &seconds.to_be_bytes())?;
            }
            (Adapter::MchoseMouse, Action::Dpi(dpi))
                if (50..=26000).contains(&dpi) && dpi % 50 == 0 =>
            {
                let cfg = self.config()?;
                self.mchose(0x12, 0x40, &cfg.dpi_arguments(dpi, current.wired), false)?;
            }
            (Adapter::MchoseMouse, Action::Rate(rate))
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
            (Adapter::MchoseMouse, Action::Sleep(seconds))
                if (60..=3600).contains(&seconds) && seconds % 60 == 0 =>
            {
                self.mchose(0x11, 0x0a, &[1, (seconds / 60) as u8], false)?;
            }
            (Adapter::MchoseMouse, Action::Stage(stage))
                if (stage as usize) < current.dpi_stages.len() =>
            {
                let cfg = self.config()?;
                let mut args = cfg.dpi_arguments(cfg.dpi(current.wired), current.wired);
                args[if current.wired { 0 } else { 1 }] = stage;
                self.mchose(0x12, 0x40, &args, false)?;
            }
            (Adapter::MchoseMouse, Action::Tune(Tuning::Debounce, v)) if v <= 20 => {
                self.mchose(0x11, 0x43, &[v as u8], false)?;
            }
            (Adapter::MchoseMouse, Action::Tune(key, v)) => {
                let cfg = self.config()?;
                let args = protocol::mchose_tuning_arguments(&cfg, key, v)?;
                self.mchose(0x11, 0x42, &args, false)?;
            }
            _ => return Err("参数不在该设备的已知范围内".into()),
        }
        thread::sleep(Duration::from_millis(180));
        let check = self.read()?;
        let (actual, expected) = match action {
            Action::Dpi(v) => (check.dpi, v),
            Action::Rate(v) => (check.rate, v),
            Action::Sleep(v) => (check.sleep_seconds, v),
            Action::Tune(key, v) => (check.tuning.get(&key).copied(), v),
            Action::Stage(v) => (check.active_stage.map(u16::from), u16::from(v)),
            Action::Light(_) => unreachable!(),
        };
        if actual != Some(expected) {
            return Err(format!(
                "已发送，但读回未确认：期望 {expected}，实际 {actual:?}。设备尚未给出有效确认，请重新读取"
            ));
        }
        Ok(())
    }
}

impl super::DeviceSession for Channel {
    fn read(&self) -> Result<State> {
        Channel::read(self)
    }
    fn apply(&self, action: Action) -> Result<()> {
        Channel::apply(self, action)
    }
}
