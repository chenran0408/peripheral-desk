use super::*;

// Indices are never recycled during the process. A tray click delivered after
// a hotplug must still name the original unit, even if it has gone away.
pub(super) struct Unit {
    model: usize,
    id: String,
    pub(super) name: String,
    state: backend::State,
    attached: bool,
    selected: bool,
}

impl App {
    pub(super) fn sync_units(&mut self, model: usize) {
        let spec = &REGISTRY[model];
        let targets = crate::plugin_manager::device_targets(spec.key);
        let selected = crate::plugin_manager::selected_device_target(spec.key);
        for unit in self.units.iter_mut().filter(|u| u.model == model) {
            unit.attached = false;
            unit.selected = selected.as_ref().is_some_and(|t| t.id == unit.id);
            unit.state.fresh = false;
            unit.state.present = false;
            unit.state.charging = None;
        }
        let multiple = targets.len() > 1;
        for target in targets {
            let active = selected
                .as_ref()
                .is_some_and(|chosen| chosen.id == target.id);
            let state = crate::plugin_manager::device_target_state(spec.key, &target.id)
                .unwrap_or_else(|| backend::State {
                    key: spec.key.to_owned(),
                    present: true,
                    ..Default::default()
                });
            let name = if multiple {
                format!("{} · {}", spec.name, target.label)
            } else {
                spec.name.to_owned()
            };
            let position = self
                .units
                .iter()
                .position(|u| u.model == model && u.id == target.id);
            let unit = Unit {
                model,
                id: target.id,
                name,
                state,
                attached: true,
                selected: active,
            };
            if let Some(position) = position {
                self.units[position] = unit;
            } else {
                self.units.push(unit);
            }
        }
    }

    pub(super) fn expand_units(&self, entries: Vec<Command>) -> Vec<Command> {
        let mut expanded = Vec::new();
        for entry in entries {
            let (Command::Battery(model) | Command::Device(model)) = entry else {
                expanded.push(entry);
                continue;
            };
            let start = expanded.len();
            for (idx, _) in self
                .units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.model == model && u.attached)
            {
                expanded.push(if matches!(entry, Command::Battery(_)) {
                    Command::UnitBattery(idx)
                } else {
                    Command::UnitDevice(idx)
                });
            }
            if expanded.len() == start {
                expanded.push(entry);
            }
        }
        expanded
    }

    pub(super) fn unit_view(&self, command: Command) -> Option<(usize, &str, &backend::State)> {
        match command {
            Command::Device(model) | Command::Battery(model) => {
                let name = self
                    .units
                    .iter()
                    .find(|u| u.model == model && u.selected)
                    .map_or(REGISTRY.get(model)?.name, |u| u.name.as_str());
                Some((model, name, self.snapshot.devices.get(model)?))
            }
            Command::UnitDevice(i) | Command::UnitBattery(i) => {
                let unit = self.units.get(i)?;
                Some((unit.model, &unit.name, &unit.state))
            }
            _ => None,
        }
    }

    pub(super) unsafe fn select_unit(&mut self, hwnd: HWND, index: usize) -> Option<usize> {
        let unit = self.units.get(index)?;
        let model = unit.model;
        if !unit.selected
            && (self.board.saving && self.board.model == Some(model)
                || self.queued.iter().any(|(m, _)| *m == model)
                || self.inflight.is_some_and(|(m, _)| m == model)
                || crate::controls_ui::writing_model(model))
        {
            self.set_status("这台设备还有修改正在保存", false);
            self.render(hwnd);
            return None;
        }
        if let Err(error) =
            crate::plugin_manager::select_device_target(REGISTRY[model].key, &unit.id)
        {
            self.set_status(&error, true);
            self.render(hwnd);
            return None;
        }
        if !unit.selected {
            self.pointer_sample = None;
            self.board.result = None;
        }
        self.snapshot.devices[model] = unit.state.clone();
        for (i, other) in self
            .units
            .iter_mut()
            .enumerate()
            .filter(|(_, u)| u.model == model)
        {
            other.selected = i == index;
        }
        self.poll_skip.insert(model);
        Some(model)
    }
}
