use super::{App, Command, REGISTRY};
use crate::devices::DeviceSpec;

impl App {
    pub(super) fn battery_entries(&self) -> Vec<Command> {
        let entries = battery_entries(
            REGISTRY.iter().map(|spec| spec.supports("battery")),
            self.snapshot.system_devices.len(),
        );
        self.expand_units(entries)
    }

    pub(super) fn driver_entries(&self) -> Vec<Command> {
        let entries = REGISTRY
            .iter()
            .enumerate()
            .filter(|(_, spec)| spec.mchose() || has_native_controls(spec))
            .map(|(m, _)| Command::Device(m))
            .collect();
        let mut entries = self.expand_units(entries);
        entries.sort_by_key(|cmd| {
            self.unit_view(*cmd).map_or(2, |(_, _, s)| {
                if s.fresh {
                    0
                } else if s.present {
                    1
                } else {
                    2
                }
            })
        });
        entries
    }
}

fn battery_entries(capabilities: impl Iterator<Item = bool>, system_count: usize) -> Vec<Command> {
    capabilities
        .enumerate()
        .filter_map(|(index, battery)| battery.then_some(Command::Battery(index)))
        .chain((0..system_count).map(Command::System))
        .collect()
}

pub(super) fn has_native_controls(spec: &DeviceSpec) -> bool {
    spec.has_g2_audio()
        || !spec.read_only()
        || spec.switch_only()
        || [
            "lighting-editor",
            "bindings",
            "key-colors",
            "onboard-profiles",
            "pointer-controls",
            "dpi-stages-read",
            "polling-rate-read",
        ]
        .iter()
        .any(|capability| spec.supports(capability))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_list_uses_declared_capabilities_before_a_first_read() {
        let entries = battery_entries([true, false, true, false].into_iter(), 2);
        assert_eq!(
            entries,
            [
                Command::Battery(0),
                Command::Battery(2),
                Command::System(0),
                Command::System(1)
            ]
        );
    }
}
