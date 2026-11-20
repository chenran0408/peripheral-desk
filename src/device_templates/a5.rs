//! Original A5 Ultra / 0x67 performance fields with a reversible single-setting API.
use crate::model::{Action, State, Tuning};
use serde_json::{json, Value};

const TUNING: [Tuning; 7] = [
    Tuning::Debounce,
    Tuning::Lod,
    Tuning::Motion,
    Tuning::Angle,
    Tuning::Ripple,
    Tuning::EsportsMode,
    Tuning::SensorRotation,
];

pub(super) fn capture(state: &State) -> Value {
    let actions: Vec<_> = state
        .sleep_seconds
        .map(Action::Sleep)
        .into_iter()
        .chain(
            TUNING
                .into_iter()
                .filter_map(|k| Some(Action::Tune(k, *state.tuning.get(&k)?))),
        )
        .collect();
    json!(actions)
}

pub(super) fn desired(document: &Value) -> Vec<Action> {
    if document["format"] == super::FORMAT {
        return document["performance"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .filter(|a| {
                matches!(a, Action::Sleep(_))
                    || matches!(a, Action::Tune(k, _) if TUNING.contains(k))
            })
            .collect();
    }
    let Some(info) = document
        .as_object()
        .and_then(|d| d.values().next())
        .and_then(|r| r["allInfo"].as_object())
    else {
        return Vec::new();
    };
    let byte = |field: &str| info.get(field)?.as_u64().and_then(|v| u8::try_from(v).ok());
    let mut actions = Vec::new();
    if let Some(m) = byte("sleep").filter(|n| *n <= 60) {
        actions.push(Action::Sleep(u16::from(m) * 60));
    }
    if let Some(v) = byte("keyDebounce").filter(|n| *n <= 20) {
        actions.push(Action::Tune(Tuning::Debounce, u16::from(v)));
    }
    if let Some(v) = byte("sensor") {
        if matches!(v & 3, 1 | 2) {
            actions.push(Action::Tune(Tuning::Lod, u16::from(v & 3)));
        }
        for (key, mask) in [
            (Tuning::Ripple, 4),
            (Tuning::Angle, 8),
            (Tuning::Motion, 16),
        ] {
            actions.push(Action::Tune(key, u16::from(v & mask != 0)));
        }
        if v & 0x40 == 0 {
            actions.push(Action::Tune(Tuning::EsportsMode, u16::from(v & 0x80 != 0)));
        }
    }
    if let Some(rotation) = info.get("rotateVal").and_then(Value::as_i64).and_then(|v| {
        if (-30..=30).contains(&v) {
            Some(v)
        } else {
            u8::try_from(v)
                .ok()
                .map(|b| i64::from(b as i8))
                .filter(|i| (-30..=30).contains(i))
        }
    }) {
        actions.push(Action::Tune(Tuning::SensorRotation, (rotation + 30) as u16));
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vendor_fields_match_native_settings_without_patching_dpi_or_unknown_modes() {
        let doc = json!({"A5 Ultra":{"name":"桌面","allInfo":{"sensor":0x95,"keyDebounce":4,"sleep":10,"rotateVal":246,"dpi0":600}}});
        let actions = desired(&doc);
        assert!(actions.contains(&Action::Tune(Tuning::Motion, 1)));
        assert!(actions.contains(&Action::Tune(Tuning::SensorRotation, 20)));
        assert!(actions.contains(&Action::Tune(Tuning::EsportsMode, 1)));
        let mut state = State::default();
        for action in &actions {
            match *action {
                Action::Sleep(s) => state.sleep_seconds = Some(s),
                Action::Tune(k, v) => {
                    state.tuning.insert(k, v);
                }
                _ => panic!("unexpected field"),
            }
        }
        let native = json!({"format":super::super::FORMAT,"performance":capture(&state)});
        let roundtrip = desired(&native);
        assert_eq!(actions.len(), roundtrip.len());
        assert!(actions.iter().all(|a| roundtrip.contains(a)));
    }
}
