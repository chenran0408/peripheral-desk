use crate::{
    devices::REGISTRY,
    hid::{DeviceTarget, Interface},
    model::State,
    protocol::Result,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
};

#[derive(Default)]
struct Inventory {
    connected: HashMap<String, Vec<DeviceTarget>>,
    selected: HashMap<String, DeviceTarget>,
    samples: HashMap<(String, String), State>,
}
static INVENTORY: LazyLock<Mutex<Inventory>> = LazyLock::new(Default::default);
type Gate = Arc<Mutex<()>>;
static GATES: LazyLock<Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>> =
    LazyLock::new(Default::default);
static WEB: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

pub(crate) fn observe_interfaces(interfaces: &[Interface]) {
    let mut routes: HashMap<String, BTreeMap<String, DeviceTarget>> = HashMap::new();
    for iface in interfaces {
        let mut target = None;
        for spec in REGISTRY.iter().filter(|s| s.matches(iface)) {
            let t = target.get_or_insert_with(|| iface.target());
            routes
                .entry(spec.key.into())
                .or_default()
                .entry(t.id.clone())
                .or_insert_with(|| t.clone());
        }
    }
    let mut data = INVENTORY.lock().unwrap();
    data.connected = routes
        .into_iter()
        .map(|(k, ts)| (k, ts.into_values().collect()))
        .collect();
    let Inventory {
        connected,
        selected,
        samples,
    } = &mut *data;
    for (key, available) in connected.iter() {
        if let [only] = available.as_slice() {
            selected.entry(key.clone()).or_insert_with(|| only.clone());
        }
    }
    for ((key, id), s) in samples.iter_mut() {
        if !connected
            .get(key)
            .is_some_and(|ts| ts.iter().any(|t| &t.id == id))
        {
            s.fresh = false;
            s.present = false;
            s.charging = None;
        }
    }
}

pub fn device_targets(key: &str) -> Vec<DeviceTarget> {
    INVENTORY
        .lock()
        .unwrap()
        .connected
        .get(key)
        .cloned()
        .unwrap_or_default()
}
pub fn selected_device_target(key: &str) -> Option<DeviceTarget> {
    INVENTORY.lock().unwrap().selected.get(key).cloned()
}
pub fn select_device_target(key: &str, id: &str) -> Result<()> {
    let mut inventory = INVENTORY.lock().unwrap();
    let target = inventory
        .connected
        .get(key)
        .and_then(|ts| ts.iter().find(|t| t.id == id))
        .cloned()
        .ok_or("所选设备已断开，请刷新设备列表")?;
    inventory.selected.insert(key.to_owned(), target);
    Ok(())
}
pub fn device_target_state(key: &str, id: &str) -> Option<State> {
    INVENTORY
        .lock()
        .unwrap()
        .samples
        .get(&(key.to_owned(), id.to_owned()))
        .cloned()
}
pub(crate) fn remember(key: &str, target: &DeviceTarget, state: &State) {
    let mut inventory = INVENTORY.lock().unwrap();
    if inventory
        .connected
        .get(key)
        .is_some_and(|ts| ts.iter().any(|t| t.id == target.id))
    {
        inventory
            .samples
            .insert((key.to_owned(), target.id.clone()), state.clone());
    }
}

thread_local! {
    static EDITOR: RefCell<Option<(String, Option<DeviceTarget>)>> = const { RefCell::new(None) };
}
/// Bind a compound operation (including readback) to the editor's captured receiver.
pub fn with_device_target<T>(key: &str, target: Option<&DeviceTarget>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<(String, Option<DeviceTarget>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            EDITOR.replace(self.0.take());
        }
    }
    let _restore = Restore(EDITOR.replace(Some((key.to_owned(), target.cloned()))));
    f()
}
pub(crate) fn bound(key: &str) -> Option<Option<DeviceTarget>> {
    EDITOR.with_borrow(|scope| {
        scope
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, t)| t.clone())
    })
}

pub(super) fn gate(id: &str) -> Gate {
    let mut gates = GATES.lock().unwrap();
    if let Some(gate) = gates.get(id).and_then(|weak| weak.upgrade()) {
        return gate;
    }
    gates.retain(|_, weak| weak.strong_count() > 0);
    let gate = Arc::new(Mutex::new(()));
    gates.insert(id.to_owned(), Arc::downgrade(&gate));
    gate
}
pub fn web_owns_instance(id: &str) -> bool {
    WEB.lock().unwrap().contains(id)
}
pub fn claim_web_driver_instance(id: &str) -> Result<()> {
    // Once pending, new plugin reads must yield. Wait only for the current transaction.
    WEB.lock().unwrap().insert(id.to_owned());
    let gate = gate(id);
    drop(gate.lock().unwrap());
    Ok(())
}
pub fn release_web_driver_instance(id: &str) {
    WEB.lock().unwrap().remove(id);
}
pub fn set_web_driver_instances(ids: Vec<String>) {
    *WEB.lock().unwrap() = ids.into_iter().collect();
}

pub(super) fn resolve(
    available: &[DeviceTarget],
    requested: Option<&DeviceTarget>,
    addressed: bool,
) -> Result<Option<DeviceTarget>> {
    if !addressed && available.len() > 1 {
        return Err(
            "检测到多台匹配此插件的设备，当前插件无法区分，请更新插件或仅连接目标设备".into(),
        );
    }
    if let Some(target) = requested {
        return available
            .iter()
            .find(|t| t.id == target.id)
            .cloned()
            .map(Some)
            .ok_or_else(|| "所选设备已断开，本次操作未转交其他设备".to_string());
    }
    match available {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        _ => Err("请先选择要操作的那台设备".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_lease_waits_for_a_write_on_that_receiver_only() {
        let a = "a-test-receiver";
        let b = "b-test-receiver";
        let gate = gate(a);
        let write = gate.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let web = std::thread::spawn(move || {
            claim_web_driver_instance(a).unwrap();
            tx.send(()).unwrap();
        });
        // The independent keyboard remains available while the mouse write drains.
        assert!(super::gate(b).try_lock().is_ok());
        assert!(!web_owns_instance(b));
        assert!(rx.try_recv().is_err());
        drop(write);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(web_owns_instance(a));
        release_web_driver_instance(a);
        assert!(!web_owns_instance(a));
        web.join().unwrap();
    }

    #[test]
    fn same_model_receivers_bind_to_the_selected_instance() {
        let a = DeviceTarget {
            id: "receiver1".into(),
            label: "A".into(),
        };
        let b = DeviceTarget {
            id: "receiver2".into(),
            label: "B".into(),
        };
        let both = [a.clone(), b.clone()];
        let second = resolve(&both, Some(&b), true).unwrap().unwrap();
        assert_eq!(second, b);
        assert!(resolve(&both, Some(&b), false).is_err());
        assert!(resolve(&[a], Some(&b), true).is_err());
        assert!(resolve(&both, None, true).is_err());
    }
}
