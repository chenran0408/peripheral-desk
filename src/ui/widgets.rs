//! Shared design tokens and reusable widget interaction state.
#![allow(dead_code)]
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, SystemTime},
};

pub(crate) const SPACE_1: i32 = 4;
pub(crate) const SPACE_2: i32 = 8;
pub(crate) const SPACE_3: i32 = 12;
pub(crate) const SPACE_4: i32 = 16;
pub(crate) const SPACE_5: i32 = 24;
pub(crate) const SPACE_6: i32 = 32;

pub(crate) const RADIUS_SM: i32 = 6;
pub(crate) const RADIUS_MD: i32 = 10;
pub(crate) const RADIUS_LG: i32 = 14;

pub(crate) const FONT_XS: i32 = 11;
pub(crate) const FONT_SM: i32 = 12;
pub(crate) const FONT_MD: i32 = 14;
pub(crate) const FONT_LG: i32 = 18;
pub(crate) const FONT_XL: i32 = 24;

pub(crate) const HOVER: u32 = 0xff1c1c24;
pub(crate) const PRESSED: u32 = 0xff23232c;
pub(crate) const SHADOW_1: u32 = 0x08000000;
pub(crate) const SHADOW_2: u32 = 0x10000000;
pub(crate) const SHADOW_3: u32 = 0x18000000;
pub(crate) const SUCCESS: u32 = crate::paint::GREEN;
pub(crate) const WARN: u32 = crate::paint::AMBER;
pub(crate) const DANGER: u32 = crate::paint::RED;

#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    start: SystemTime,
    duration: Duration,
}

impl Tween {
    fn progress(self, now: SystemTime) -> f32 {
        if self.duration.is_zero() {
            return 1.;
        }
        let elapsed = now.duration_since(self.start).unwrap_or_default();
        (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0., 1.)
    }

    fn value(self, now: SystemTime) -> f32 {
        let progress = ease_out_cubic(self.progress(now));
        self.from + (self.to - self.from) * progress
    }

    fn active(self, now: SystemTime) -> bool {
        self.progress(now) < 1.
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

#[derive(Clone, Copy, Debug)]
struct RingTween {
    tween: Tween,
    visible: bool,
}

#[derive(Default)]
pub(crate) struct WidgetFx {
    hovered: HashSet<u64>,
    pressed: Option<u64>,
    tweens: HashMap<u64, Tween>,
    rings: HashMap<u64, RingTween>,
}

impl WidgetFx {
    pub(crate) fn hover(&self, key: u64) -> bool {
        self.hovered.contains(&key)
    }

    pub(crate) fn set_hover(&mut self, key: u64, hover: bool) -> bool {
        if hover {
            self.hovered.insert(key)
        } else {
            self.hovered.remove(&key)
        }
    }

    pub(crate) fn press(&mut self, key: u64) {
        self.pressed = Some(key);
    }

    pub(crate) fn pressed(&self, key: u64) -> bool {
        self.pressed == Some(key)
    }

    pub(crate) fn clear_press(&mut self) {
        self.pressed = None;
    }

    pub(crate) fn animate(&mut self, key: u64, to: f32, dur_ms: u32) {
        let now = SystemTime::now();
        let from = self.value_at(key, now).unwrap_or(to);
        self.tweens.insert(
            key,
            Tween {
                from,
                to,
                start: now,
                duration: Duration::from_millis(dur_ms as u64),
            },
        );
    }

    pub(crate) fn value(&self, key: u64) -> f32 {
        self.value_at(key, SystemTime::now()).unwrap_or(0.)
    }

    fn value_at(&self, key: u64, now: SystemTime) -> Option<f32> {
        self.tweens.get(&key).map(|tween| tween.value(now))
    }

    pub(crate) fn ring_target(&mut self, key: u64, percent: Option<u8>) {
        let Some(percent) = percent else {
            if let Some(ring) = self.rings.get_mut(&key) {
                ring.visible = false;
            }
            return;
        };
        let now = SystemTime::now();
        let target = percent.min(100) as f32;
        let current = self
            .rings
            .get(&key)
            .map(|ring| ring.tween.value(now))
            .unwrap_or(target);
        self.rings.insert(
            key,
            RingTween {
                tween: Tween {
                    from: current,
                    to: target,
                    start: now,
                    duration: Duration::from_millis(600),
                },
                visible: true,
            },
        );
    }

    pub(crate) fn ring_value(&self, key: u64) -> Option<f32> {
        let ring = self.rings.get(&key)?;
        ring.visible.then(|| ring.tween.value(SystemTime::now()))
    }

    pub(crate) fn is_active(&self) -> bool {
        let now = SystemTime::now();
        self.tweens.values().any(|tween| tween.active(now))
            || self.rings.values().any(|ring| ring.tween.active(now))
    }

    /// Advance animations; return true while any animation is still running.
    pub(crate) fn tick(&mut self, _hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        self.is_active()
    }

    pub(crate) fn on_move(&mut self, _x: i32, _y: i32) {}

    pub(crate) fn on_leave(&mut self) {
        self.hovered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_has_expected_endpoints() {
        assert_eq!(ease_out_cubic(0.), 0.);
        assert_eq!(ease_out_cubic(1.), 1.);
        assert_eq!(ease_out_cubic(0.5), 0.875);
    }

    #[test]
    fn tween_advances_and_finishes() {
        let start = SystemTime::now();
        let tween = Tween {
            from: 10.,
            to: 90.,
            start,
            duration: Duration::from_millis(100),
        };
        assert_eq!(tween.value(start), 10.);
        assert!((tween.value(start + Duration::from_millis(50)) - 80.).abs() < 0.001);
        assert_eq!(tween.value(start + Duration::from_millis(100)), 90.);
        assert!(!tween.active(start + Duration::from_millis(100)));
    }

    #[test]
    fn hover_registry_reports_changes() {
        let mut fx = WidgetFx::default();
        assert!(!fx.hover(7));
        assert!(fx.set_hover(7, true));
        assert!(fx.hover(7));
        assert!(!fx.set_hover(7, true));
        assert!(fx.set_hover(7, false));
        assert!(!fx.hover(7));
    }
}
