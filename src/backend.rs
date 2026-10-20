use crate::{
    hid::{self, Interface},
    protocol::Result,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::devices::{Adapter, REGISTRY};
pub use crate::model::{Action, State};

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub devices: Vec<State>,
    pub interval: u32,
    pub threshold: u8,
    pub notices: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            devices: REGISTRY
                .iter()
                .map(|spec| State {
                    key: spec.key.into(),
                    ..State::default()
                })
                .collect(),
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
        let saved = std::mem::take(&mut s.devices);
        s.devices = REGISTRY
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let mut state = saved
                    .iter()
                    .find(|d| d.key == spec.key)
                    .cloned()
                    .or_else(|| saved.get(index).filter(|d| d.key.is_empty()).cloned())
                    .unwrap_or_default();
                state.key = spec.key.into();
                state
            })
            .collect();
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
            d.light = None;
            d.tuning.clear();
            d.dpi_stages.clear();
            d.active_stage = None;
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
    (REGISTRY[model].matcher)(i)
}

pub fn refresh(previous: &Snapshot, action: Option<(usize, Action)>) -> (Snapshot, Result<()>) {
    refresh_with(previous, action, hid::enumerate(), crate::adapters::open)
}

fn refresh_with<F>(
    previous: &Snapshot,
    action: Option<(usize, Action)>,
    interfaces: Result<Vec<Interface>>,
    mut open: F,
) -> (Snapshot, Result<()>)
where
    F: FnMut(usize, &Interface) -> Result<Box<dyn crate::adapters::DeviceSession>>,
{
    let mut next = previous.clone();
    if action.is_some_and(|(model, _)| model >= REGISTRY.len()) {
        return (next, Err("设备编号无效".into()));
    }
    let interfaces = match interfaces {
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
    for (model, spec) in REGISTRY.iter().enumerate() {
        let mut candidates: Vec<_> = interfaces.iter().filter(|i| matches(i, model)).collect();
        candidates.sort_by_key(|i| {
            if spec.adapter == Adapter::Razer && i.pid == 0xb6 {
                0
            } else {
                1
            }
        });
        let d = &mut next.devices[model];
        d.present = !candidates.is_empty();
        d.fresh = false;
        d.charging = None;
        d.dpi = None;
        d.rate = None;
        d.sleep_seconds = None;
        d.light = None;
        d.tuning.clear();
        d.dpi_stages.clear();
        d.active_stage = None;
        if candidates.is_empty() {
            d.error = "未检测到支持的接口，请连接接收器或 USB 数据线".into();
            if action.is_some_and(|a| a.0 == model) {
                action_result = Err(d.error.clone());
            }
            continue;
        }
        let mut read = None;
        for interface in candidates {
            let result = open(model, interface).and_then(|session| {
                if let Some((m, a)) = action {
                    if m == model {
                        action_result = session.read().and_then(|current| {
                            spec.validate(a, &current)?;
                            session.apply(a)
                        });
                    }
                }
                session.read()
            });
            match result {
                Ok(mut s) => {
                    s.key = spec.key.into();
                    s.alerted = d.alerted;
                    read = Some(s);
                    break;
                }
                Err(e) => d.error = e,
            }
        }
        if let Some(s) = read {
            *d = s;
        } else if action.is_some_and(|a| a.0 == model) {
            action_result = Err(d.error.clone());
        }
    }
    (next, action_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::mouse::Channel;
    use std::{thread, time::Duration};
    #[test]
    #[ignore = "Requires all three physical peripherals; writes the existing values and checks read-back"]
    fn live_settings_readback() {
        let all = hid::enumerate().unwrap();
        let mut results = Vec::new();
        for (model, spec) in REGISTRY.iter().enumerate() {
            let name = spec.name;
            let i = all
                .iter()
                .find(|i| matches(i, model))
                .expect("Required peripheral missing");
            if spec.adapter != Adapter::CompxKeyboard {
                let channel = Channel::new(i, spec).unwrap();
                let before = channel.read().unwrap();
                let actions = [
                    Action::Dpi(before.dpi.unwrap()),
                    Action::Rate(before.rate.unwrap()),
                    Action::Sleep(before.sleep_seconds.unwrap()),
                ];
                for action in actions {
                    thread::sleep(Duration::from_millis(450));
                    let result = channel.apply(action);
                    let line = format!("{name} {action:?}: {result:?}");
                    println!("{line}");
                    results.push(line);
                }
            } else {
                let k = crate::keyboard::Keyboard::open(&i.path).unwrap();
                let before = k.read(format!("{:04X}:{:04X}", i.vid, i.pid)).unwrap();
                let actions = [
                    Action::Rate(before.rate.unwrap()),
                    Action::Sleep(before.sleep_seconds.unwrap()),
                    Action::Light(before.light.unwrap()),
                ];
                for action in actions {
                    thread::sleep(Duration::from_millis(450));
                    let result = k.apply(action);
                    let line = format!("{name} {action:?}: {result:?}");
                    println!("{line}");
                    results.push(line);
                }
            }
        }
        for result in &results {
            println!("{result}");
        }
        assert!(
            results.iter().all(|r| r.ends_with("Ok(())")),
            "Hardware read-back failed"
        );
    }
    #[test]
    #[ignore = "Requires awake physical devices; writes only current setting values"]
    fn live_extended_readback() {
        let all = hid::enumerate().unwrap();
        for (model, spec) in REGISTRY.iter().enumerate() {
            let interface = all
                .iter()
                .find(|i| matches(i, model))
                .expect("Required device missing");
            let session = crate::adapters::open(model, interface).unwrap();
            let state = session.read().unwrap();
            let mut actions = Vec::new();
            if let Some(stage) = state.active_stage {
                actions.push(Action::Stage(stage));
            }
            for setting in spec.advanced {
                if let Some(value) = state.tuning.get(&setting.key) {
                    actions.push(Action::Tune(setting.key, *value));
                }
            }
            assert!(!actions.is_empty());
            for action in actions {
                let result = session.apply(action);
                println!("{} {:?}: {:?}", spec.name, action, result);
                assert!(result.is_ok());
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    #[test]
    #[ignore = "Changes then restores settings on awake physical devices"]
    fn live_feature_roundtrips_restore_original_values() {
        fn roundtrip(
            session: &dyn crate::adapters::DeviceSession,
            change: Action,
            original: Action,
        ) {
            let changed = session.apply(change);
            let restored = session.apply(original);
            println!(
                "roundtrip {:?}: {:?}; restore {:?}: {:?}",
                change, changed, original, restored
            );
            assert!(changed.is_ok() && restored.is_ok());
        }
        let all = hid::enumerate().unwrap();
        for (model, spec) in REGISTRY.iter().enumerate() {
            let interface = all.iter().find(|i| matches(i, model)).unwrap();
            let session = crate::adapters::open(model, interface).unwrap();
            let before = session.read().unwrap();
            if let Some(dpi) = before.dpi {
                roundtrip(
                    &*session,
                    Action::Dpi(if dpi == 1600 { 1650 } else { 1600 }),
                    Action::Dpi(dpi),
                );
            }
            if let Some(stage) = before.active_stage {
                let other = if stage == 0 { 1 } else { 0 };
                roundtrip(&*session, Action::Stage(other), Action::Stage(stage));
            }
            for setting in spec.advanced {
                if let Some(value) = before.tuning.get(&setting.key) {
                    if let Some((other, _)) = setting.choices.iter().find(|(v, _)| v != value) {
                        roundtrip(
                            &*session,
                            Action::Tune(setting.key, *other),
                            Action::Tune(setting.key, *value),
                        );
                    }
                }
            }
            if spec.adapter == Adapter::CompxKeyboard {
                let original = before.light.unwrap();
                let enable = session.apply(Action::Light(2));
                let checked = (|| -> Result<()> {
                    enable?;
                    let active = session.read()?;
                    for key in [
                        crate::model::Tuning::Brightness,
                        crate::model::Tuning::Speed,
                    ] {
                        let value = *active.tuning.get(&key).ok_or("灯效未返回参数")?;
                        let other = if value == 1 { 2 } else { 1 };
                        let changed = session.apply(Action::Tune(key, other));
                        let restored = session.apply(Action::Tune(key, value));
                        println!(
                            "keyboard {:?} {} -> {}: {:?}; restore: {:?}",
                            key, value, other, changed, restored
                        );
                        changed?;
                        restored?;
                    }
                    Ok(())
                })();
                let restore = session.apply(Action::Light(original));
                assert!(checked.is_ok(), "{checked:?}");
                assert!(restore.is_ok(), "{restore:?}");
            }
        }
    }
    struct FakeSession {
        state: State,
        writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }
    impl crate::adapters::DeviceSession for FakeSession {
        fn read(&self) -> Result<State> {
            Ok(self.state.clone())
        }
        fn apply(&self, _: Action) -> Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail {
                Err("read-back mismatch".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn unplug_keeps_cached_battery_but_disables_settings() {
        let mut previous = Snapshot::default();
        previous.devices[0].fresh = true;
        previous.devices[0].percent = Some(90);
        previous.devices[0].dpi = Some(1600);
        let (next, result) = refresh_with(&previous, None, Ok(vec![]), |_, _| {
            panic!("must not open absent device")
        });
        assert!(result.is_ok());
        assert_eq!(next.devices[0].percent, Some(90));
        assert!(!next.devices[0].fresh);
        assert!(!next.devices[0].present);
        assert_eq!(next.devices[0].dpi, None);
    }
    #[test]
    fn failed_write_is_reported_while_fresh_status_is_kept() {
        let i = Interface {
            path: "mi_00".into(),
            vid: 0x1532,
            pid: 0xb7,
            usage_page: 1,
            usage: 2,
            feature_len: 91,
            input_len: 0,
            output_len: 0,
            product: "test".into(),
        };
        let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = writes.clone();
        let (next, result) = refresh_with(
            &Snapshot::default(),
            Some((0, Action::Dpi(1600))),
            Ok(vec![i]),
            move |_, _| {
                Ok(Box::new(FakeSession {
                    state: State {
                        present: true,
                        fresh: true,
                        percent: Some(90),
                        dpi: Some(1600),
                        ..State::default()
                    },
                    writes: counter.clone(),
                    fail: true,
                }))
            },
        );
        assert!(result.is_err());
        assert!(next.devices[0].fresh);
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!next.devices[1].present);
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
