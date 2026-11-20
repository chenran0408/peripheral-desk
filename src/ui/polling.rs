//! Background battery reads never own the foreground editor's operation slot.
use super::*;

impl App {
    pub(super) unsafe fn start_poll(&mut self, hwnd: HWND) {
        if self.polling.is_some() {
            return;
        }
        self.poll_skip.clear();
        self.poll_skip.extend(self.inflight.map(|(m, _)| m));
        self.poll_skip.extend(self.queued.iter().map(|(m, _)| *m));
        let editing = match self.page {
            Page::Tool(m, _) => Some(m),
            _ => (0..REGISTRY.len()).find(|&m| crate::controls_ui::writing_model(m)),
        };
        self.poll_skip.extend(editing);
        let (tx, rx) = mpsc::channel();
        self.polling = Some(rx);
        let before = self.snapshot.clone();
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = backend::refresh_progress_except(&before, editing, |model, state| {
                if tx.send(WorkUpdate::Device(model, state)).is_ok() {
                    PostMessageW(window as HWND, POLL_DONE, 0, 0);
                }
            });
            if tx.send(WorkUpdate::Finished(result)).is_ok() {
                PostMessageW(window as HWND, POLL_DONE, 0, 0);
            }
        });
    }

    pub(super) unsafe fn complete_poll(&mut self, hwnd: HWND) {
        let Some(rx) = self.polling.as_ref() else {
            return;
        };
        let (updates, disconnected) = drain_work_updates(rx);
        let mut done = disconnected;
        let mut changed = false;
        for update in updates {
            match update {
                WorkUpdate::Device(model, mut state) if !self.poll_skip.contains(&model) => {
                    state.alerted = self.snapshot.devices[model].alerted;
                    self.snapshot.devices[model] = state;
                    self.sync_units(model);
                    changed = true;
                }
                WorkUpdate::Finished((result, poll_result)) => {
                    let selected = match self.page {
                        Page::System(index) => self
                            .snapshot
                            .system_devices
                            .get(index)
                            .map(|s| s.state.key.clone()),
                        _ => None,
                    };
                    self.snapshot.system_devices = result.system_devices;
                    if let Some(key) = selected {
                        self.page = self
                            .snapshot
                            .system_devices
                            .iter()
                            .position(|d| d.state.key == key)
                            .map(Page::System)
                            .unwrap_or(Page::Home);
                    }
                    if let Err(error) = poll_result {
                        // If enumeration itself failed the backend sent no individual results.
                        for (i, state) in result.devices.into_iter().enumerate() {
                            if !state.fresh && !self.poll_skip.contains(&i) {
                                self.snapshot.devices[i] = state;
                            }
                        }
                        self.set_status(&format!("设备刷新失败：{error}"), true);
                    } else if self.status == "正在读取设备…" {
                        self.set_status("设备状态已更新", false);
                    }
                    changed = true;
                    done = true;
                }
                _ => {}
            }
        }
        if done {
            self.polling = None;
            for m in self.snapshot.alerts() {
                self.notify(hwnd, m);
            }
            if let Err(error) = self.snapshot.save() {
                self.set_status(&format!("保存偏好失败：{error}"), true);
            }
        }
        if changed {
            self.update_ring_targets();
            self.tray(hwnd);
            if matches!(self.page, Page::Home | Page::Drivers) {
                self.layout(hwnd);
            } else {
                self.render(hwnd);
            }
            if !self.flyout.is_null() {
                InvalidateRect(self.flyout, null(), 0);
            }
        }
        if done && std::mem::take(&mut self.poll_again) {
            self.start_poll(hwnd);
        }
        self.poll_detail_on_entry(hwnd);
    }
}
