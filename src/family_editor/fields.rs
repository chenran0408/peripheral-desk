use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub(super) struct Section {
    pub id: String,
    pub label: String,
}
#[derive(Deserialize)]
pub(super) struct Choice {
    pub value: i32,
    pub label: String,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub w: Option<f32>,
    pub h: Option<f32>,
}
#[derive(Deserialize)]
pub(super) struct Selector {
    pub label: String,
    pub value: i32,
    pub choices: Vec<Choice>,
}
#[derive(Deserialize)]
pub(super) struct Field {
    pub id: String,
    pub label: String,
    pub value: i32,
    pub min: Option<i32>,
    pub max: Option<i32>,
    pub step: Option<i32>,
    #[serde(default = "one")]
    pub scale: u32,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub readonly: bool,
}
fn one() -> u32 {
    1
}
pub(super) type Changes = BTreeMap<String, i32>;

impl Field {
    pub fn display(&self, raw: i32) -> String {
        let digits = self.scale.max(1).ilog10() as usize;
        format!(
            "{:.*}",
            digits,
            f64::from(raw) / f64::from(self.scale.max(1))
        )
    }
    pub fn parse(&self, text: &str) -> Result<i32, String> {
        let invalid = || format!("{}：请输入有效的数值", self.label);
        let value: f64 = text.trim().parse().map_err(|_| invalid())?;
        let raw = value * f64::from(self.scale.max(1));
        if !raw.is_finite()
            || !(i32::MIN as f64..=i32::MAX as f64).contains(&raw)
            || (raw - raw.round()).abs() > 0.000001
        {
            return Err(invalid());
        }
        let raw = raw.round() as i32;
        if self.min.is_some_and(|min| raw < min) || self.max.is_some_and(|max| raw > max) {
            return Err(format!("{} 超出设备范围", self.label));
        }
        if let Some(step) = self.step.filter(|v| *v > 1) {
            if (i64::from(raw) - i64::from(self.min.unwrap_or(0))) % i64::from(step) != 0 {
                return Err(format!(
                    "{} 的步长为 {} {}",
                    self.label,
                    self.display(step),
                    self.unit
                ));
            }
        }
        Ok(raw)
    }
}

#[derive(Deserialize)]
pub(super) struct CurvePoint {
    pub value: i32,
    pub label: String,
    pub db: f32,
}
#[derive(Deserialize, Default)]
pub(super) struct DevicePage {
    pub sections: Vec<Section>,
    pub section: String,
    pub selector: Option<Selector>,
    pub scope: Option<Selector>,
    pub profile: Option<u32>,
    #[serde(default)]
    pub curve: Vec<CurvePoint>,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub note: String,
}
impl DevicePage {
    pub fn title(&self) -> &str {
        self.sections
            .iter()
            .find(|s| s.id == self.section)
            .map_or("", |s| s.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_device_precision_and_step_are_accepted() {
        let field: Field = serde_json::from_value(serde_json::json!({"id":"rt","label":"触发行程","value":25,"min":5,"max":400,"step":5,"scale":100})).unwrap();
        assert_eq!(field.display(25), "0.25");
        assert_eq!(field.parse("0.30").unwrap(), 30);
        for invalid in ["0.32", "0.251", "0", "4.05", "NaN"] {
            assert!(field.parse(invalid).is_err());
        }
    }
}
