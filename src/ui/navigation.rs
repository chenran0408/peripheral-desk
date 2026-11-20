use super::{brand::Brand, App, Command, REGISTRY};
use crate::devices::DeviceSpec;

impl App {
    pub(super) fn battery_entries(&self) -> Vec<Command> {
        battery_entries(
            REGISTRY.iter().map(|spec| spec.supports("battery")),
            self.snapshot.system_devices.len(),
        )
    }

    pub(super) fn driver_entries(&self) -> Vec<Command> {
        driver_entries(&REGISTRY)
    }

    pub(super) fn brand_devices(&self, brand: Brand) -> Vec<usize> {
        brand_devices(&REGISTRY, brand)
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
    !spec.mchose()
        && (spec.has_g2_audio()
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
            .any(|capability| spec.supports(capability)))
}

fn driver_entries(registry: &[DeviceSpec]) -> Vec<Command> {
    [
        Command::MHub,
        Command::Brand(Brand::Razer),
        Command::Brand(Brand::Logitech),
    ]
    .into_iter()
    .chain(registry.iter().enumerate().filter_map(|(index, spec)| {
        (has_native_controls(spec) && Brand::for_device(spec).is_none())
            .then_some(Command::Device(index))
    }))
    .collect()
}

fn brand_devices(registry: &[DeviceSpec], brand: Brand) -> Vec<usize> {
    registry
        .iter()
        .enumerate()
        .filter_map(|(index, spec)| {
            (Brand::for_device(spec) == Some(brand) && has_native_controls(spec)).then_some(index)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::DeviceManifest;

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

    #[test]
    fn brand_workspaces_are_unique_and_only_list_models_with_controls() {
        let specs: Vec<_> = [
            ("mchose-a5-ultra", "MCHOSE", "mouse"),
            ("mchose-g75-pro", "迈从", "keyboard"),
            ("readonly-mouse", "Other", "mouse"),
            ("hecate-g2-pro", "HECATE", "headset"),
            ("adjustable-mouse", "Other", "mouse"),
            ("adjustable-razer", "RAZER", "mouse"),
            ("battery-razer", "Razer", "mouse"),
            ("adjustable-logitech", "Logitech", "mouse"),
            ("logitech-astro", "ASTRO", "headset"),
        ]
        .into_iter()
        .map(|(key, brand, kind)| {
            let mut value = serde_json::json!({
                "key":key, "name":key, "brand":brand, "kind":kind, "matches":[{"vid":1,"pid":2}]
            });
            if key == "mchose-a5-ultra" || key.starts_with("adjustable-") {
                value["dpi_range"] = serde_json::json!([100, 20000]);
            }
            serde_json::from_value::<DeviceManifest>(value)
                .unwrap()
                .into_spec()
                .unwrap()
        })
        .collect();
        let hubs = [
            Command::MHub,
            Command::Brand(Brand::Razer),
            Command::Brand(Brand::Logitech),
        ];
        assert_eq!(driver_entries(&[]), hubs);
        assert_eq!(brand_devices(&specs, Brand::Razer), [5]);
        assert_eq!(brand_devices(&specs, Brand::Logitech), [7]);
        assert_eq!(Brand::for_device(&specs[8]), None);
        assert_eq!(
            driver_entries(&specs),
            [
                hubs[0],
                hubs[1],
                hubs[2],
                Command::Device(3),
                Command::Device(4)
            ]
        );
    }
}
