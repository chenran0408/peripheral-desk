use crate::{
    hid::{self, Interface},
    protocol::Result,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(all(test, feature = "driver-runtime"))]
use crate::devices::Adapter;
use crate::devices::REGISTRY;
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
        s.devices = states_for_registry(&saved, &REGISTRY);
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
        crate::storage::write_json_preserving(
            &data_path(),
            serde_json::to_value(self).map_err(|e| e.to_string())?,
        )
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
    REGISTRY.get(model).is_some_and(|spec| spec.matches(i))
}

fn states_for_registry(saved: &[State], registry: &[crate::devices::DeviceSpec]) -> Vec<State> {
    let mut states = Vec::with_capacity(registry.len());
    for spec in registry {
        let keyed = saved.iter().find(|state| state.key == spec.key).cloned();
        // Positional migration is meaningful only for the fixed legacy SDK
        // roots. A desktop user's plugin set may have any size or order.
        #[cfg(feature = "driver-runtime")]
        let keyed = keyed.or_else(|| {
            saved
                .get(states.len())
                .filter(|state| state.key.is_empty())
                .cloned()
        });
        let mut state = keyed.unwrap_or_default();
        state.key = spec.key.into();
        states.push(state);
    }
    states
}

pub fn refresh(previous: &Snapshot, action: Option<(usize, Action)>) -> (Snapshot, Result<()>) {
    refresh_with(previous, action, hid::enumerate(), crate::adapters::open)
}

pub fn refresh_progress<F>(previous: &Snapshot, mut progress: F) -> (Snapshot, Result<()>)
where
    F: FnMut(usize, State),
{
    let interfaces = match hid::enumerate() {
        Ok(interfaces) => interfaces,
        Err(error) => {
            return refresh_scoped(previous, None, None, Err(error), crate::adapters::open)
        }
    };
    refresh_parallel(previous, interfaces, crate::adapters::open, &mut progress)
}

fn refresh_parallel<F, P>(
    previous: &Snapshot,
    interfaces: Vec<Interface>,
    open: F,
    progress: &mut P,
) -> (Snapshot, Result<()>)
where
    F: Fn(usize, &Interface) -> Result<Box<dyn crate::adapters::DeviceSession>> + Sync,
    P: FnMut(usize, State),
{
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };

    let mut next = previous.clone();
    next.devices = states_for_registry(&previous.devices, &REGISTRY);
    let interfaces = Arc::new(interfaces);

    // Serialize models that expose the same VID/PID. This conservatively keeps
    // sibling HID collections on a shared receiver from being read together.
    let identities: Vec<std::collections::HashSet<_>> = REGISTRY
        .iter()
        .enumerate()
        .map(|(model, _)| {
            interfaces
                .iter()
                .filter(|interface| matches(interface, model))
                .map(|interface| (interface.vid, interface.pid))
                .collect()
        })
        .collect();
    let mut parents: Vec<usize> = (0..REGISTRY.len()).collect();
    for left in 0..REGISTRY.len() {
        for right in (left + 1)..REGISTRY.len() {
            if identities[left]
                .iter()
                .any(|id| identities[right].contains(id))
            {
                let left_root = root(&mut parents, left);
                let right_root = root(&mut parents, right);
                parents[right_root] = left_root;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for model in 0..REGISTRY.len() {
        let model_root = root(&mut parents, model);
        if let Some(group) = groups
            .iter_mut()
            .find(|group| root(&mut parents, group[0]) == model_root)
        {
            group.push(model);
        } else {
            groups.push(vec![model]);
        }
    }

    let group_index = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let prior_devices = Arc::new(next.devices.clone());
    let workers = std::thread::available_parallelism()
        .map_or(2, |count| count.get().clamp(2, 4))
        .min(4)
        .min(groups.len().max(1));
    let result = std::thread::scope(|scope| {
        for _ in 0..workers {
            let groups = &groups;
            let interfaces = interfaces.clone();
            let group_index = group_index.clone();
            let tx = tx.clone();
            let open = &open;
            let prior_devices = prior_devices.clone();
            scope.spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
                    let group = group_index.fetch_add(1, Ordering::Relaxed);
                    let Some(models) = groups.get(group) else {
                        break;
                    };
                    for &model in models {
                        let state =
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                refresh_scoped(
                                    previous,
                                    None,
                                    Some(model),
                                    Ok((*interfaces).clone()),
                                    |index, interface| open(index, interface),
                                )
                                .0
                                .devices[model]
                                    .clone()
                            })) {
                                Ok(state) => state,
                                Err(_) => {
                                    let mut state = prior_devices[model].clone();
                                    state.fresh = false;
                                    state.charging = None;
                                    state.error = "设备读取线程发生异常".into();
                                    state
                                }
                            };
                        if tx.send((model, state)).is_err() {
                            return;
                        }
                    }
                }));
            });
        }
        drop(tx);
        let mut received = 0;
        let mut completed = vec![false; REGISTRY.len()];
        let mut result = Ok(());
        while received < REGISTRY.len() {
            match rx.recv() {
                Ok((model, state)) => {
                    next.devices[model] = state.clone();
                    progress(model, state);
                    completed[model] = true;
                    received += 1;
                }
                Err(_) => {
                    for (model, state) in next.devices.iter_mut().enumerate() {
                        if completed[model] {
                            continue;
                        }
                        state.fresh = false;
                        state.charging = None;
                        state.error = "设备读取线程已退出".into();
                    }
                    result = Err("设备读取线程已退出".into());
                    break;
                }
            }
        }
        result
    });
    (next, result)
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

pub fn refresh_device(previous: &Snapshot, model: usize) -> (Snapshot, Result<()>) {
    refresh_scoped(
        previous,
        None,
        Some(model),
        hid::enumerate(),
        crate::adapters::open,
    )
}
fn refresh_with<F>(
    previous: &Snapshot,
    action: Option<(usize, Action)>,
    interfaces: Result<Vec<Interface>>,
    open: F,
) -> (Snapshot, Result<()>)
where
    F: FnMut(usize, &Interface) -> Result<Box<dyn crate::adapters::DeviceSession>>,
{
    refresh_scoped(previous, action, action.map(|a| a.0), interfaces, open)
}
fn refresh_scoped<F>(
    previous: &Snapshot,
    action: Option<(usize, Action)>,
    scope: Option<usize>,
    interfaces: Result<Vec<Interface>>,
    mut open: F,
) -> (Snapshot, Result<()>)
where
    F: FnMut(usize, &Interface) -> Result<Box<dyn crate::adapters::DeviceSession>>,
{
    let mut next = previous.clone();
    next.devices = states_for_registry(&previous.devices, &REGISTRY);
    if action.is_some_and(|(model, _)| model >= REGISTRY.len())
        || scope.is_some_and(|model| model >= REGISTRY.len())
    {
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
        if scope.is_some_and(|target| target != model) {
            continue;
        }
        let mut candidates: Vec<_> = interfaces.iter().filter(|i| matches(i, model)).collect();
        candidates.sort_by_key(|i| {
            if spec.key == "razer-deathadder-v3-pro" && i.pid == 0xb6 {
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
            if scope == Some(model) {
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
                    if !s.fresh && s.present && s.percent.is_none() {
                        s.percent = d.percent;
                    }
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
        } else if scope == Some(model) {
            action_result = Err(d.error.clone());
        }
    }
    (next, action_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(not(feature = "driver-runtime"))]
    fn installed_registry_controls_cached_state_identity_without_positional_defaults() {
        let installed = crate::devices::host_registry_from_installed([(
            "0.4.2".into(),
            serde_json::json!({"key":"mchose-a5-ultra","name":"A5","brand":"MCHOSE","kind":"mouse","matches":[{"vid":14391,"pid":4108}]}),
        )]);
        let saved = vec![
            State {
                percent: Some(99),
                ..State::default()
            },
            State {
                key: "mchose-a5-ultra".into(),
                percent: Some(78),
                ..State::default()
            },
        ];
        assert!(states_for_registry(&saved, &[]).is_empty());
        let selected = states_for_registry(&saved, &installed);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].key, "mchose-a5-ultra");
        assert_eq!(selected[0].percent, Some(78));
        let unkeyed_only = states_for_registry(&saved[..1], &installed);
        assert_eq!(
            unkeyed_only[0].percent, None,
            "never reuse another model's old positional cache"
        );
    }
    #[cfg(feature = "driver-runtime")]
    use crate::adapters::mouse::Channel;
    #[cfg(feature = "driver-runtime")]
    use std::{thread, time::Duration};
    #[test]
    #[ignore = "Requires all three physical peripherals; writes the existing values and checks read-back"]
    #[cfg(feature = "driver-runtime")]
    fn live_settings_readback() {
        let all = hid::enumerate().unwrap();
        let mut results = Vec::new();
        for (model, spec) in REGISTRY.iter().enumerate() {
            if spec.adapter == Adapter::HecateAudio {
                continue;
            }
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
    #[cfg(feature = "driver-runtime")]
    fn live_extended_readback() {
        let all = hid::enumerate().unwrap();
        for (model, spec) in REGISTRY.iter().enumerate() {
            if spec.adapter == Adapter::HecateAudio {
                continue;
            }
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
    #[cfg(feature = "driver-runtime")]
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
            if spec.adapter == Adapter::HecateAudio {
                continue;
            }
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
                if setting.key == crate::model::Tuning::DpiStages
                    || setting.key == crate::model::Tuning::MacMode
                {
                    continue;
                }
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
    fn parallel_refresh_reports_fast_device_before_slow_device_and_keeps_all_states() {
        let mut previous = Snapshot::default();
        previous.devices[2].percent = Some(43);
        previous.devices[2].updated = 99;
        let interfaces = vec![
            Interface {
                path: "mi_00-deathadder".into(),
                vid: 0x1532,
                pid: 0xb7,
                usage_page: 1,
                usage: 2,
                feature_len: 91,
                input_len: 0,
                output_len: 0,
                product: "DeathAdder".into(),
            },
            Interface {
                path: "a5".into(),
                vid: 0x3837,
                pid: 0x100c,
                usage_page: 0xff01,
                usage: 1,
                feature_len: 65,
                input_len: 65,
                output_len: 65,
                product: "A5 ultra".into(),
            },
        ];
        let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = writes.clone();
        let fast_reported = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wait_for_fast = fast_reported.clone();
        let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mark_timeout = timed_out.clone();
        let mut progress = Vec::new();
        let (next, result) = refresh_parallel(
            &previous,
            interfaces,
            move |model, _| {
                if model == 0 {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    while !wait_for_fast.load(std::sync::atomic::Ordering::Acquire)
                        && std::time::Instant::now() < deadline
                    {
                        std::thread::yield_now();
                    }
                    if !wait_for_fast.load(std::sync::atomic::Ordering::Acquire) {
                        mark_timeout.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                Ok(Box::new(FakeSession {
                    state: State {
                        present: true,
                        fresh: true,
                        percent: Some(if model == 0 { 80 } else { 70 }),
                        ..State::default()
                    },
                    writes: counter.clone(),
                    fail: false,
                }))
            },
            &mut |model, state| {
                if model == 1 {
                    fast_reported.store(true, std::sync::atomic::Ordering::Release);
                }
                progress.push((model, state.percent));
            },
        );
        assert!(result.is_ok());
        assert!(!timed_out.load(std::sync::atomic::Ordering::Acquire));
        let fast_position = progress.iter().position(|(model, _)| *model == 1).unwrap();
        let slow_position = progress.iter().position(|(model, _)| *model == 0).unwrap();
        assert!(fast_position < slow_position);
        assert_eq!(next.devices[0].percent, Some(80));
        assert_eq!(next.devices[1].percent, Some(70));
        assert_eq!(next.devices[2].percent, Some(43));
        assert_eq!(next.devices[2].updated, 99);
    }

    #[test]
    fn setting_refresh_probes_only_target_and_keeps_other_timestamps() {
        let mut previous = Snapshot::default();
        previous.devices[1].fresh = true;
        previous.devices[1].present = true;
        previous.devices[1].percent = Some(88);
        previous.devices[1].updated = 123;
        let interfaces = vec![
            Interface {
                path: "mi_00".into(),
                vid: 0x1532,
                pid: 0xb7,
                usage_page: 1,
                usage: 2,
                feature_len: 91,
                input_len: 0,
                output_len: 0,
                product: "test".into(),
            },
            Interface {
                path: "a5".into(),
                vid: 0x3837,
                pid: 0x100c,
                usage_page: 0xff01,
                usage: 1,
                feature_len: 65,
                input_len: 65,
                output_len: 65,
                product: "A5 ultra".into(),
            },
        ];
        let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = writes.clone();
        let mut opens = 0;
        let (next, result) = refresh_with(
            &previous,
            Some((0, Action::Dpi(1600))),
            Ok(interfaces),
            |model, _| {
                assert_eq!(model, 0);
                opens += 1;
                Ok(Box::new(FakeSession {
                    state: State {
                        present: true,
                        fresh: true,
                        dpi: Some(1600),
                        ..State::default()
                    },
                    writes: counter.clone(),
                    fail: false,
                }))
            },
        );
        assert!(result.is_ok());
        assert_eq!(opens, 1);
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(next.devices[1].updated, 123);
        assert_eq!(next.devices[1].percent, Some(88));
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
