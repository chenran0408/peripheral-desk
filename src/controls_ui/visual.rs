//! Model artwork and physical key coordinates for the embedded native editor.
use super::*;
use crate::paint::palette;
use serde::Deserialize;
use std::{collections::BTreeSet, fs::File, io::Read};
use windows::{
    core::Interface,
    Win32::{System::Com::IStream, UI::Shell::SHCreateMemStream},
};
use windows_sys::Win32::Graphics::GdiPlus::{GdipDisposeImage, GdipLoadImageFromStream, GpImage};

pub(super) const WIDTH: i32 = 800;
pub(super) const HEIGHT: i32 = 592;
pub(super) const LAYER: i32 = 160;
pub(super) const SELECT_KEY: i32 = 161;
pub(super) const HOVER_TIMER: usize = 0x83;
const HOVER_MS: f32 = 100.;
pub(super) const STAGE: R = R {
    x: 22,
    y: 65,
    w: 756,
    h: 282,
};

#[derive(Deserialize)]
struct Hotspot {
    id: u16,
    label: String,
    rect: [i32; 4],
    #[serde(default)]
    hit_rect: Option<[i32; 4]>,
    #[serde(default)]
    anchor: Option<[i32; 2]>,
}

#[derive(Deserialize)]
struct Diagram {
    size: [i32; 2],
    #[serde(default)]
    art: Option<[i32; 4]>,
    layers: Vec<String>,
    keys: Vec<Hotspot>,
}

impl Diagram {
    fn validate(&self) -> bool {
        let [w, h] = self.size;
        if !(1..=10000).contains(&w)
            || !(1..=10000).contains(&h)
            || self.keys.is_empty()
            || self.keys.len() > 256
            || self.layers.is_empty()
            || self.layers.len() > 16
        {
            return false;
        }
        let inside = |[x, y, rw, rh]: [i32; 4]| {
            x >= 0
                && y >= 0
                && rw > 0
                && rh > 0
                && i64::from(x) + i64::from(rw) <= i64::from(w)
                && i64::from(y) + i64::from(rh) <= i64::from(h)
        };
        let mut seen = BTreeSet::new();
        self.art.is_none_or(inside)
            && self.keys.iter().all(|k| {
                seen.insert(k.id)
                    && inside(k.rect)
                    && k.hit_rect.is_none_or(inside)
                    && !k.label.is_empty()
                    && k.anchor
                        .is_none_or(|[x, y]| (0..w).contains(&x) && (0..h).contains(&y))
            })
    }
    fn project(&self, [x, y, w, h]: [i32; 4]) -> R {
        let [sw, sh] = self.size;
        let scale = ((STAGE.w - 20) as f32 / sw as f32).min((STAGE.h - 14) as f32 / sh as f32);
        let sx = STAGE.x + (STAGE.w - (sw as f32 * scale).round() as i32) / 2;
        let sy = STAGE.y + (STAGE.h - (sh as f32 * scale).round() as i32) / 2;
        let at = |value: i32| (value as f32 * scale).round() as i32;
        R::new(sx + at(x), sy + at(y), at(x + w) - at(x), at(y + h) - at(y))
    }
    fn hit(&self, x: i32, y: i32) -> Option<u16> {
        self.keys
            .iter()
            .find(|k| {
                self.project(k.rect).contains(x, y)
                    || k.hit_rect.is_some_and(|r| self.project(r).contains(x, y))
            })
            .map(|k| k.id)
    }
}

struct Artwork {
    image: *mut GpImage,
    _stream: IStream,
}
impl Drop for Artwork {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.image);
        }
    }
}
impl Artwork {
    fn load(model: &str, g75: bool) -> Option<Self> {
        let bytes = if g75 {
            let package = std::env::current_exe()
                .ok()?
                .parent()?
                .join("mhub-builtin/payload.zip");
            let mut zip = zip::ZipArchive::new(File::open(package).ok()?).ok()?;
            let mut bytes = Vec::new();
            zip.by_name(
                "vendor-cdn/configCenter/static/images/G75ProNorthSnowFull_95d97fd8065c.png",
            )
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
            bytes
        } else {
            std::fs::read(crate::plugin_manager::artwork(model)?).ok()?
        };
        unsafe {
            let stream = SHCreateMemStream(Some(&bytes))?;
            let mut image = null_mut();
            if GdipLoadImageFromStream(stream.as_raw(), &mut image) != 0 {
                return None;
            }
            Some(Self {
                image,
                _stream: stream,
            })
        }
    }
}

pub(super) struct Visual {
    diagram: Diagram,
    art: Option<Artwork>,
    layer: u8,
    hover: Option<u16>,
    previous_hover: Option<u16>,
    transition: Option<std::time::Instant>,
    animate_hover: bool,
    drag: Option<(u16, i32, i32)>,
}
impl Visual {
    pub(super) fn load(model: &str) -> Option<Self> {
        let declared = crate::plugin_manager::installed(model)
            .ok()
            .flatten()
            .and_then(|d| d.manifest.device.get("control_surface").cloned());
        let g75 = model == "mchose-g75-pro";
        let diagram: Diagram = if let Some(layout) = declared {
            serde_json::from_value(layout).ok()?
        } else if g75 {
            serde_json::from_str(include_str!("layouts/g75-pro.json")).ok()?
        } else if model == "mchose-a5-ultra" {
            serde_json::from_str(include_str!("layouts/a5-ultra.json")).ok()?
        } else {
            return None;
        };
        if !diagram.validate() {
            return None;
        }
        let art = Artwork::load(model, g75);
        let mut animations: i32 = 0;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut animations as *mut i32).cast(),
                0,
            );
        }

        Some(Self {
            diagram,
            art,
            layer: 0,
            hover: None,
            previous_hover: None,
            transition: None,
            animate_hover: animations != 0,
            drag: None,
        })
    }
    fn hover_progress(&self) -> f32 {
        self.transition.map_or(1., |start| {
            (start.elapsed().as_secs_f32() * 1000. / HOVER_MS).min(1.)
        })
    }
    unsafe fn hover_at(&mut self, hwnd: HWND, hit: Option<u16>) {
        if self.hover == hit {
            return;
        }
        self.previous_hover = self.hover;
        self.hover = hit;
        if self.animate_hover {
            self.transition = Some(std::time::Instant::now());
            SetTimer(hwnd, HOVER_TIMER, 16, None);
        }
        InvalidateRect(hwnd, null(), 0);
    }
    pub(super) unsafe fn tick_hover(&mut self, hwnd: HWND) {
        if self.hover_progress() >= 1. {
            self.transition = None;
            self.previous_hover = None;
            KillTimer(hwnd, HOVER_TIMER);
        }
        InvalidateRect(hwnd, null(), 0);
    }
    fn selected(&self, index: usize) -> Option<u16> {
        self.diagram.keys.get(index).map(|k| k.id)
    }
}

impl Editor {
    pub(super) unsafe fn visual_layout(&mut self, hwnd: HWND) {
        let visual = self.visual.as_ref().unwrap();
        if self.page == 0 {
            if let Some(state) = &self.state {
                if !state
                    .bindings
                    .get(self.selected)
                    .is_some_and(|b| b.layer == visual.layer)
                {
                    self.selected = state
                        .bindings
                        .iter()
                        .position(|b| b.layer == visual.layer)
                        .unwrap_or(usize::MAX);
                }
            }
        }
        let layers = visual.diagram.layers.clone();
        let layer = usize::from(visual.layer);
        let available: Vec<_> = visual
            .diagram
            .keys
            .iter()
            .map(|k| k.label.clone())
            .collect();
        let selected_control = self.visual_selected();
        let selected_index = visual
            .diagram
            .keys
            .iter()
            .position(|k| k.id == selected_control)
            .unwrap_or(usize::MAX);
        if self.page == 0 && layers.len() > 1 {
            self.combo(hwnd, LAYER, R::new(510, 20, 95, 180), &layers);
            self.set_select(LAYER, layer);
        }
        if self.macro_available() {
            self.button(hwnd, 11, "板载宏", R::new(411, 15, 86, 32));
        }
        if REGISTRY[self.model].supports("key-colors") {
            self.button(hwnd, 10, "改键", R::new(614, 15, 74, 32));
            self.button(hwnd, 12, "逐键灯", R::new(694, 15, 84, 32));
        }
        self.combo(hwnd, SELECT_KEY, R::new(24, 385, 162, 270), &available);
        self.set_select(SELECT_KEY, selected_index);
        if self.page == 0 {
            let labels = [
                "鼠标按键",
                "键盘按键 / 组合键",
                "多媒体",
                "禁用此键",
                "DPI 循环",
                "升高一档 DPI",
                "降低一档 DPI",
            ];
            let kinds = self
                .kinds()
                .into_iter()
                .map(|n| labels[n].to_owned())
                .collect::<Vec<_>>();
            self.combo(hwnd, KIND, R::new(201, 385, 172, 245), &kinds);
            self.combo(hwnd, TARGET, R::new(388, 385, 196, 245), &[]);
            for (i, label) in ["Ctrl", "Shift", "Alt", "Win"].into_iter().enumerate() {
                self.add(
                    hwnd,
                    MODS[i],
                    "BUTTON",
                    label,
                    R::new(200 + i as i32 * 72, 426, 70, 25),
                    BS_AUTOCHECKBOX as u32,
                );
            }
            self.button(hwnd, APPLY, "应用到此键", R::new(637, 380, 140, 37));
            self.sync_binding();
        } else {
            self.button(hwnd, PICK, "选择颜色…", R::new(201, 380, 132, 37));
            self.button(hwnd, APPLY, "应用到此键", R::new(488, 380, 133, 37));
            self.button(hwnd, ALL, "应用到全部键", R::new(634, 380, 143, 37));
            self.sync_color();
        }
        self.button(hwnd, RELOAD, "重新读取", R::new(658, 530, 120, 35));
        self.add(
            hwnd,
            STATUS,
            "EDIT",
            &self.status.clone(),
            R::new(24, 518, 615, 54),
            ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
        );
        for child in &self.children {
            ShowWindow(child.hwnd, SW_SHOWNA);
        }
        self.availability();
        InvalidateRect(hwnd, null(), 0);
    }
    pub(super) fn visual_selected(&self) -> u16 {
        if self.page == 2 {
            self.state
                .as_ref()
                .and_then(|s| s.colors.get(self.selected))
                .map(|k| k.control)
        } else {
            self.binding().map(|b| b.control)
        }
        .unwrap_or(u16::MAX)
    }
    pub(super) unsafe fn select_hotspot(&mut self, hwnd: HWND, id: u16) -> bool {
        let Some(v) = &self.visual else { return false };
        let index = self.state.as_ref().and_then(|s| {
            if self.page == 2 {
                s.colors.iter().position(|c| c.control == id)
            } else {
                s.bindings
                    .iter()
                    .position(|b| b.control == id && b.layer == v.layer)
            }
        });
        let Some(i) = index else {
            self.status(hwnd, "尚未读到这个按键的板载值。", true);
            return false;
        };
        self.selected = i;
        if let Some(i) = v.diagram.keys.iter().position(|k| k.id == id) {
            self.set_select(SELECT_KEY, i);
        }
        if self.page == 2 {
            self.sync_color()
        } else {
            self.sync_binding()
        }
        InvalidateRect(hwnd, null(), 0);
        true
    }
    pub(super) fn visual_section(&self) -> Option<ControlSection> {
        let visual = self.visual.as_ref()?;
        let scoped = REGISTRY[self.model].supports("control-sections");
        #[cfg(test)]
        let scoped = scoped || self.test_device.is_some();
        if !scoped {
            return None;
        }
        match self.page {
            0 => Some(ControlSection::Layer(visual.layer)),
            1 if REGISTRY[self.model].supports("macro-section") => Some(ControlSection::Macros),
            2 => Some(ControlSection::Colors),
            _ => None,
        }
    }
    pub(super) unsafe fn show_visual_section(&mut self, hwnd: HWND, selected: u16) {
        if self.visual_section().is_some_and(|section| {
            !self.state.as_ref().is_some_and(|s| match section {
                ControlSection::Layer(layer) => s.bindings.iter().any(|b| b.layer == layer),
                ControlSection::Colors => !s.colors.is_empty(),
                ControlSection::Macros => s.macro_supported,
            })
        }) {
            self.restore_selection = (selected != u16::MAX).then_some(selected);
            self.state = None;
            self.selected = usize::MAX;
            self.begin(hwnd, None);
        } else if selected != u16::MAX {
            self.select_hotspot(hwnd, selected);
        }
    }
    pub(super) unsafe fn visual_command(&mut self, hwnd: HWND, id: i32, event: u32) -> bool {
        if self.visual.is_none() {
            return false;
        }
        match id {
            SELECT_KEY if event == CBN_SELCHANGE => {
                if let Some(id) = self
                    .visual
                    .as_ref()
                    .and_then(|v| v.selected(self.select(SELECT_KEY)))
                {
                    self.select_hotspot(hwnd, id);
                }
            }
            LAYER if event == CBN_SELCHANGE && self.pending.is_none() => {
                let old = self.visual_selected();
                let layer = self.select(LAYER) as u8;
                self.visual.as_mut().unwrap().layer = layer;
                self.show_visual_section(hwnd, old);
            }
            10..=12 => self.change_page(hwnd, (id - 10) as usize),
            _ => return false,
        }
        true
    }
    pub(super) unsafe fn visual_pointer(&mut self, hwnd: HWND, msg: u32, l: LPARAM) -> bool {
        let x = (l as i16 as i32) * 96 / self.dpi as i32;
        let y = ((l >> 16) as i16 as i32) * 96 / self.dpi as i32;
        let Some(v) = self.visual.as_mut() else {
            return false;
        };
        let hit = v.diagram.hit(x, y);
        match msg {
            WM_MOUSEMOVE => {
                v.hover_at(hwnd, hit);
                if hit.is_some() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as _,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        ..zeroed()
                    };
                    TrackMouseEvent(&mut track);
                }
            }
            WM_MOUSELEAVE => {
                v.hover_at(hwnd, None);
            }
            WM_LBUTTONDOWN => {
                if let Some(key) = hit {
                    v.drag = Some((key, x, y));
                    SetCapture(hwnd);
                    self.select_hotspot(hwnd, key);
                }
            }
            WM_LBUTTONUP => {
                let drag = v.drag.take();
                let layer = v.layer;
                if drag.is_some() {
                    ReleaseCapture();
                }
                if let (Some((from, sx, sy)), Some(to)) = (drag, hit) {
                    if self.page == 0 && from != to && (x.abs_diff(sx) > 3 || y.abs_diff(sy) > 3) {
                        let action = self
                            .state
                            .as_ref()
                            .and_then(|s| {
                                s.bindings
                                    .iter()
                                    .find(|b| b.control == from && b.layer == layer)
                            })
                            .map(|b| b.action.clone());
                        if let Some(a) = action.filter(|a| {
                            !matches!(a, BindingAction::Unknown(_) | BindingAction::Macro(_))
                        }) {
                            if self.select_hotspot(hwnd, to) && self.editable() {
                                self.begin(
                                    hwnd,
                                    Some(ControlAction::Bind {
                                        control: to,
                                        layer,
                                        action: a,
                                    }),
                                );
                            }
                        }
                    }
                }
            }
            WM_CAPTURECHANGED => {
                v.drag = None;
            }
            _ => return false,
        }
        true
    }
    pub(super) unsafe fn paint_visual(&self, c: &Canvas) {
        let v = self.visual.as_ref().unwrap();
        let d = &v.diagram;
        let selected = self.visual_selected();
        c.rect(R::new(0, 0, WIDTH, HEIGHT), palette().bg);
        c.text(
            REGISTRY[self.model].name,
            R::new(24, 12, 430, 36),
            24,
            600,
            palette().text,
            DT_LEFT,
        );
        c.round(STAGE, 14, palette().panel, Some(palette().border));
        if let Some(art) = &v.art {
            let [sw, sh] = d.size;
            c.image(art.image, d.project(d.art.unwrap_or([0, 0, sw, sh])), true);
        }
        let transition = v.hover_progress();
        for k in &d.keys {
            let r = d.project(k.rect);
            let callout = d.art.is_some();
            if let Some([ax, ay]) = k.anchor {
                let anchor = d.project([ax, ay, 1, 1]);
                let edge = if anchor.x > r.x + r.w { r.x + r.w } else { r.x };
                c.line(edge, r.y + r.h / 2, anchor.x, anchor.y, palette().muted, 1.);
                c.ellipse(
                    R::new(anchor.x - 3, anchor.y - 3, 6, 6),
                    if selected == k.id || v.hover == Some(k.id) {
                        palette().accent
                    } else {
                        palette().muted
                    },
                );
            }
            let active = k.id == selected && self.state.is_some();
            let hover = if v.hover == Some(k.id) {
                transition
            } else if v.previous_hover == Some(k.id) {
                1. - transition
            } else {
                0.
            };
            let color = self
                .state
                .as_ref()
                .and_then(|s| s.colors.iter().find(|c| c.control == k.id));
            let bg = if active {
                (0x99 << 24) | (palette().accent & 0x00ffffff)
            } else if hover > 0. && !callout && v.art.is_some() {
                (((119. * hover) as u32) << 24) | (palette().accent_light & 0x00ffffff)
            } else if hover > 0. {
                paint::blend(palette().field, palette().accent_border, hover)
            } else if callout || v.art.is_none() {
                palette().field
            } else {
                0
            };
            if bg != 0 {
                c.round(
                    r,
                    4,
                    bg,
                    Some(if active {
                        palette().accent
                    } else if hover > 0. {
                        paint::blend(palette().border, palette().accent_light, hover)
                    } else {
                        palette().border
                    }),
                );
            }
            if self.page == 2 {
                if let Some(color) = color {
                    let [r8, g8, b8] = color.rgb;
                    c.round(
                        R::new(r.x + 2, r.y + r.h - 5, r.w - 4, 4),
                        2,
                        0xff000000 | (u32::from(r8) << 16) | (u32::from(g8) << 8) | u32::from(b8),
                        None,
                    );
                }
            }
            if callout || v.art.is_none() {
                c.text(
                    &k.label,
                    r,
                    if callout { 13 } else { 10 },
                    500,
                    palette().text,
                    DT_CENTER,
                );
            }
        }
        let fields = if self.page == 0 {
            [(24, "物理按键"), (201, "动作类型"), (388, "目标")].as_slice()
        } else {
            [(24, "选择要着色的键")].as_slice()
        };
        for (x, label) in fields {
            c.text(
                label,
                R::new(*x, 353, 170, 28),
                13,
                500,
                palette().muted,
                DT_LEFT,
            );
        }
        let hint = if self.page == 2 {
            if self.picked {
                format!(
                    "选中颜色   #{:02X}{:02X}{:02X}",
                    self.rgb[0], self.rgb[1], self.rgb[2]
                )
            } else {
                "正在等待读取逐键颜色".to_owned()
            }
        } else if let Some(b) = self.binding() {
            format!(
                "{}   ·   {}     拖动一个键到另一个键可复制其当前动作。",
                b.label,
                self.describe(&b.action)
            )
        } else {
            "正在读取当前层的板载键位…".to_owned()
        };
        c.text(
            &hint,
            R::new(24, 470, 750, 31),
            14,
            400,
            palette().muted,
            DT_LEFT,
        );
        if self.page == 2 && self.picked {
            let [r, g, b] = self.rgb;
            c.round(
                R::new(350, 382, 50, 32),
                7,
                0xff000000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
                Some(palette().border),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagrams_project_every_physical_key_to_its_protocol_slot() {
        for source in [
            include_str!("layouts/g75-pro.json"),
            include_str!("layouts/a5-ultra.json"),
        ] {
            let model: Diagram = serde_json::from_str(source).unwrap();
            assert!(model.validate());
            for key in &model.keys {
                let r = model.project(key.rect);
                assert_eq!(model.hit(r.x + r.w / 2, r.y + r.h / 2), Some(key.id));
                if let Some(physical) = key.hit_rect {
                    let r = model.project(physical);
                    assert_eq!(model.hit(r.x + r.w / 2, r.y + r.h / 2), Some(key.id));
                }
            }
        }
    }
    #[cfg(feature = "driver-runtime")]
    #[test]
    fn two_native_drags_finish_after_leaving_and_survive_reopening() {
        use crate::controls::Binding;
        use std::{
            sync::{LazyLock, Mutex},
            time::{Duration, Instant},
        };
        static DEVICE: LazyLock<Mutex<ControlState>> = LazyLock::new(|| {
            Mutex::new(ControlState {
                binding_types: vec!["keyboard".into()],
                bindings: (0..=2)
                    .flat_map(|layer| {
                        [(8, "Q", 20), (14, "W", 26), (20, "E", 8)].into_iter().map(
                            move |(control, label, usage)| Binding {
                                control,
                                layer,
                                label: label.to_owned(),
                                editable: true,
                                action: BindingAction::Keyboard {
                                    usage: usage + u16::from(layer),
                                    modifiers: 0,
                                },
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            })
        });
        fn device(
            _: usize,
            _: Option<&crate::hid::DeviceTarget>,
            _: Option<u8>,
            section: Option<ControlSection>,
            action: Option<ControlAction>,
        ) -> Job {
            let mut s = DEVICE.lock().unwrap();
            if let Some(ControlAction::Bind {
                control,
                layer,
                action,
            }) = action
            {
                s.bindings
                    .iter_mut()
                    .find(|b| b.control == control && b.layer == layer)
                    .unwrap()
                    .action = action;
            }
            let mut snapshot = s.clone();
            if let Some(ControlSection::Layer(layer)) = section {
                snapshot.bindings.retain(|b| b.layer == layer);
                snapshot.colors.clear();
            }
            Ok(snapshot)
        }
        unsafe fn pump_until(mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !done() {
                assert!(Instant::now() < deadline);
                let mut msg = zeroed();
                if PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                } else {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
        unsafe fn editor(hwnd: HWND) -> *mut Editor {
            GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor
        }
        unsafe {
            let model = REGISTRY
                .iter()
                .position(|s| s.key == "mchose-g75-pro")
                .unwrap();
            let owner = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                900,
                700,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!owner.is_null());
            let bounds = RECT {
                left: 0,
                top: 0,
                right: 860,
                bottom: 650,
            };
            let open = || {
                open_embedded(owner, model, bounds).unwrap();
                let hwnd = GetWindow(GetWindow(owner, GW_CHILD), GW_CHILD);
                (*editor(hwnd)).test_device = Some(device);
                pump_until(|| (*editor(hwnd)).state.is_some());
                hwnd
            };
            let hwnd = open();
            let form = &mut *editor(hwnd);
            form.set_select(LAYER, 1);
            SendMessageW(
                hwnd,
                WM_COMMAND,
                ((CBN_SELCHANGE << 16) | LAYER as u32) as _,
                form.child(LAYER) as _,
            );
            pump_until(|| (*editor(hwnd)).pending.is_none());
            let point = |id| {
                let form = &*editor(hwnd);
                let d = &form.visual.as_ref().unwrap().diagram;
                let r = d.project(d.keys.iter().find(|k| k.id == id).unwrap().rect);
                ((form.px(r.y + r.h / 2) << 16) | form.px(r.x + r.w / 2)) as LPARAM
            };
            let (q, w, e) = (point(8), point(14), point(20));
            for dest in [w, e] {
                SendMessageW(hwnd, WM_LBUTTONDOWN, 0, q);
                SendMessageW(hwnd, WM_LBUTTONUP, 0, dest);
            }
            assert_eq!((*editor(hwnd)).queue.len(), 1);
            assert!(close_embedded());
            assert!(embedded::detached(hwnd));
            assert!(writing_model(model));
            pump_until(|| IsWindow(hwnd) == 0);
            for _ in 0..2 {
                let (id, text, error) = take_write_notice().unwrap();
                assert_eq!(id, model);
                assert!(!error);
                assert!(text.contains("读回确认"));
            }
            assert!(take_write_notice().is_none());
            assert!(!writing_model(model));
            let reopened = open();
            for (layer, control, usage) in [(0, 14, 26), (0, 20, 8), (1, 14, 21), (1, 20, 21)] {
                if (*editor(reopened)).visual.as_ref().unwrap().layer != layer {
                    (*editor(reopened)).set_select(LAYER, layer.into());
                    SendMessageW(
                        reopened,
                        WM_COMMAND,
                        ((CBN_SELCHANGE << 16) | LAYER as u32) as _,
                        (*editor(reopened)).child(LAYER) as _,
                    );
                    pump_until(|| (*editor(reopened)).pending.is_none());
                }
                let s = (*editor(reopened)).state.as_ref().unwrap();
                let binding = s
                    .bindings
                    .iter()
                    .find(|b| b.control == control && b.layer == layer)
                    .unwrap();
                assert_eq!(
                    binding.action,
                    BindingAction::Keyboard {
                        usage,
                        modifiers: 0
                    }
                );
            }
            assert!(close_embedded());
            DestroyWindow(owner);
        }
    }
}
