//! Existing Logitech onboard profiles and their saved DPI stages.
use super::*;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Read,
    Mode(bool),
    Profile(u16),
    Dpi(u16, u8),
}

impl Op {
    fn request(self) -> Value {
        match self {
            Self::Read => json!({"op": "read-onboard-profiles"}),
            Self::Mode(board) => json!({
                "op": "set-onboard-mode",
                "mode": if board { "onboard" } else { "host" },
            }),
            Self::Profile(id) => json!({"op": "select-onboard-profile", "profile": id}),
            Self::Dpi(id, index) => {
                json!({"op": "select-onboard-dpi", "profile": id, "index": index})
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Host,
    Onboard,
}

#[derive(Debug, Deserialize)]
struct Profile {
    id: u16,
    enabled: bool,
    dpi: [u16; 5],
}

#[derive(Debug, Deserialize)]
pub(super) struct Profiles {
    mode: Mode,
    active_profile: Option<u16>,
    active_dpi_index: Option<u8>,
    profiles: Vec<Profile>,
}

impl Profiles {
    fn active(&self) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| Some(p.id) == self.active_profile && p.enabled)
    }

    fn selected(&self, op: Op) -> bool {
        match op {
            Op::Read => false,
            Op::Mode(board) => board == (self.mode == Mode::Onboard),
            Op::Profile(id) => self.active().is_some_and(|p| p.id == id),
            Op::Dpi(id, index) => {
                self.active().is_some_and(|p| p.id == id) && self.active_dpi_index == Some(index)
            }
        }
    }

    fn allows(&self, op: Op) -> bool {
        match op {
            Op::Read | Op::Mode(_) => true,
            Op::Profile(id) => {
                self.mode == Mode::Onboard && self.profiles.iter().any(|p| p.id == id && p.enabled)
            }
            Op::Dpi(id, i) => {
                self.mode == Mode::Onboard
                    && self.active_dpi_index.is_some()
                    && self.active().is_some_and(|p| {
                        p.id == id && matches!(p.dpi.get(usize::from(i)), Some(1..=65534))
                    })
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Board {
    model: Option<usize>,
    result: Option<Result<Profiles, String>>,
    pub(super) saving: bool,
}

impl Board {
    fn state(&self) -> Option<&Profiles> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }
}

impl App {
    pub(super) fn board_selected(&self, op: Op) -> bool {
        self.board.state().is_some_and(|s| s.selected(op))
    }

    pub(super) fn board_enabled(&self, model: usize, op: Op) -> bool {
        self.page == Page::Onboard(model)
            && self.pending.is_none()
            && self.queued.is_empty()
            && !self.modal_open
            && !self.hub_controls(model)
            && (op == Op::Read
                || (self.board.model == Some(model)
                    && self.board.state().is_some_and(|s| s.allows(op))))
    }

    pub(super) unsafe fn board_layout(&mut self, hwnd: HWND, model: usize) {
        let x = SIDEBAR + 48;
        self.add(
            hwnd,
            "重新读取",
            R::new(self.width - 154, 94, 124, 38),
            GHOST,
            Command::Board(Op::Read),
        );
        if self.board.model != Some(model) || self.board.state().is_none() {
            return;
        }
        self.add(
            hwnd,
            "板载模式",
            R::new(x, 234, 142, 38),
            CHIP,
            Command::Board(Op::Mode(true)),
        );
        self.add(
            hwnd,
            "主机模式",
            R::new(x + 154, 234, 142, 38),
            CHIP,
            Command::Board(Op::Mode(false)),
        );
        let state = self.board.state().unwrap();
        if state.mode == Mode::Host {
            return;
        }
        let width = ((self.width - x - 66) / 5).min(150);
        let mut slots = Vec::new();
        for (i, profile) in state.profiles.iter().enumerate() {
            let label = if profile.id > 0xff {
                format!("出厂配置 {}", i + 1)
            } else {
                format!("配置 {}", i + 1)
            };
            slots.push((
                label,
                R::new(x + i as i32 * width, 390, width - 10, 38),
                Op::Profile(profile.id),
            ));
        }
        if let Some(p) = state.active() {
            for (i, &dpi) in p
                .dpi
                .iter()
                .enumerate()
                .filter(|(_, value)| matches!(value, 1..=65534))
            {
                slots.push((
                    format!("{dpi} DPI"),
                    R::new(x + i as i32 * width, 546, width - 10, 38),
                    Op::Dpi(p.id, i as u8),
                ));
            }
        }
        for (label, rect, op) in slots {
            self.add(hwnd, &label, rect, CHIP, Command::Board(op));
        }
    }

    pub(super) unsafe fn board_paint(&self, c: &Canvas, model: usize) {
        let x = SIDEBAR + 24;
        let w = self.width - x - 28;
        c.text(
            "板载配置",
            R::new(x, 84, w - 150, 38),
            26,
            600,
            TEXT,
            DT_LEFT,
        );
        c.text(
            REGISTRY[model].name,
            R::new(x + 1, 125, w - 150, 22),
            13,
            400,
            MUTED,
            DT_LEFT,
        );
        let panel = |y, h, title: &str, hint: &str| {
            c.round(R::new(x, y, w, h), 14, PANEL, Some(BORDER));
            c.text(
                title,
                R::new(x + 24, y + 11, w - 48, 28),
                17,
                600,
                TEXT,
                DT_LEFT,
            );
            c.text(
                hint,
                R::new(x + 24, y + 39, w - 48, 25),
                13,
                400,
                MUTED,
                DT_LEFT,
            );
        };
        let result = self
            .board
            .result
            .as_ref()
            .filter(|_| self.board.model == Some(model));
        if let Some(Err(e)) = result {
            panel(
                164,
                200,
                "板载配置暂不可用",
                "可以重新读取设备，查看当前的实际配置。",
            );
            c.text(
                e,
                R::new(x + 24, 244, w - 48, 104),
                14,
                400,
                TEXT,
                DT_LEFT | DT_WORDBREAK,
            );
            return;
        }
        let Some(Ok(state)) = result else {
            panel(164, 105, "正在读取板载配置…", "保持鼠标和接收器连接。");
            return;
        };
        panel(
            164,
            126,
            "运行模式",
            if state.mode == Mode::Onboard {
                "当前由鼠标保存的板载配置控制。"
            } else {
                "当前由主机软件控制。切换到板载模式可使用鼠标中保存的配置。"
            },
        );
        if state.mode == Mode::Onboard {
            panel(
                320,
                126,
                "配置槽",
                if state.profiles.is_empty() {
                    "设备没有返回配置槽。"
                } else {
                    "选择鼠标中已启用的配置。"
                },
            );
            let hint = match state.active() {
                None => "设备未提供当前启用的配置。",
                Some(p) if !p.dpi.iter().any(|v| matches!(v, 1..=65534)) => {
                    "该配置没有报告已保存的 DPI 档位。"
                }
                Some(_) if state.active_dpi_index.is_none() => {
                    "设备未报告当前档位，可查看已保存的 DPI。"
                }
                Some(_) => "选择当前配置中已保存的 DPI 档位。",
            };
            panel(476, 126, "DPI 档位", hint);
        }
    }

    pub(super) unsafe fn board_run(&mut self, hwnd: HWND, model: usize, op: Op) {
        if !self.board_enabled(model, op) || self.board_selected(op) {
            return;
        }
        let key = REGISTRY[model].key;
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        if self.board.model != Some(model) {
            self.board.result = None;
        }
        self.board.model = Some(model);
        self.board.saving = op != Op::Read;
        self.set_status(
            if self.board.saving {
                "正在切换并从鼠标读回确认…"
            } else {
                "正在读取鼠标的板载配置…"
            },
            false,
        );
        self.render(hwnd);
        let window = hwnd as usize;
        thread::spawn(move || {
            let result = crate::plugin_manager::invoke(key, op.request()).and_then(|value| {
                serde_json::from_value(value)
                    .map_err(|e| format!("插件返回的板载配置格式不正确：{e}"))
            });
            if tx.send(WorkUpdate::Board(model, result)).is_ok() {
                PostMessageW(window as HWND, DONE, 0, 0);
            }
        });
    }

    pub(super) unsafe fn board_finished(
        &mut self,
        hwnd: HWND,
        model: usize,
        result: Result<Profiles, String>,
    ) {
        self.pending = None;
        let saved = std::mem::take(&mut self.board.saving);
        if self.page == Page::Onboard(model) && self.board.model == Some(model) {
            match &result {
                Err(e) => self.set_status(e, true),
                Ok(_) => self.set_status(
                    if saved {
                        "已切换并读回确认"
                    } else {
                        "已从鼠标读取板载配置"
                    },
                    false,
                ),
            }
            self.board.result = Some(result);
        }
        self.layout(hwnd);
        self.poll_detail_on_entry(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboard_switches_use_device_profile_ids_and_existing_dpi_indices() {
        let mut state: Profiles = serde_json::from_value(json!({
            "mode":"onboard", "active_profile":257, "active_dpi_index":2,
            "profiles":[
                {"id":257,"enabled":true,"dpi":[400,0,1600,65535,3200]},
                {"id":258,"enabled":false,"dpi":[400,800,1200,1600,2400]}
            ]
        }))
        .unwrap();
        assert!(state.selected(Op::Mode(true)));
        assert!(state.selected(Op::Profile(257)));
        assert!(state.selected(Op::Dpi(257, 2)));
        assert!(state.allows(Op::Dpi(257, 4)));
        assert_eq!(
            Op::Dpi(257, 4).request(),
            json!({"op":"select-onboard-dpi","profile":257,"index":4})
        );
        for op in [
            Op::Profile(258),
            Op::Dpi(258, 0),
            Op::Dpi(257, 1),
            Op::Dpi(257, 3),
        ] {
            assert!(!state.allows(op));
        }
        state.active_dpi_index = None;
        assert!(!state.allows(Op::Dpi(257, 0)));
        state.mode = Mode::Host;
        assert!(state.allows(Op::Mode(true)));
        assert!(!state.allows(Op::Profile(257)));
    }
}
