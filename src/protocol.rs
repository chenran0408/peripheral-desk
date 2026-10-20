//! Independently implemented from public protocol documentation; see SOURCES.md.
pub type Result<T> = std::result::Result<T, String>;

pub fn razer_request(class: u8, command: u8, size: u8, args: &[u8]) -> [u8; 91] {
    let mut wire = [0u8; 91];
    wire[2] = 0x1f;
    wire[6] = size;
    wire[7] = class;
    wire[8] = command;
    wire[9..9 + args.len()].copy_from_slice(args);
    wire[89] = wire[3..89].iter().fold(0, |a, b| a ^ b);
    wire
}

pub fn razer_reply<'a>(wire: &'a [u8], request: &[u8]) -> Result<&'a [u8]> {
    if wire.len() != 91 || wire[0] != 0 {
        return Err("雷蛇返回的报告长度或 Report ID 不匹配".into());
    }
    if wire[89] != wire[3..89].iter().fold(0, |a, b| a ^ b) {
        return Err("雷蛇报文校验失败，可能有其他驱动正在通信".into());
    }
    if wire[2] != request[2] || wire[7..9] != request[7..9] || wire[6] != request[6] {
        return Err("雷蛇返回了其他请求的报文，请关闭雷云后重试".into());
    }
    if wire[1] != 2 {
        return Err(format!("雷蛇设备未完成请求（状态 0x{:02X}）", wire[1]));
    }
    Ok(&wire[9..9 + wire[6] as usize])
}

pub fn razer_percent(raw: u8) -> u8 {
    ((raw as u16 * 100 + 127) / 255) as u8
}

/// M HUB short/long feature reports include a separate report ID byte.
/// Only meaningful bytes are inverted; padding mirrors the official sender.
pub fn mchose_request(report: u8, command: u8, args: &[u8]) -> Vec<u8> {
    let length = if report == 0x11 { 21 } else { 65 };
    let mut wire = vec![0xff; length];
    wire[0] = report;
    wire[1] = !command;
    for (dest, src) in wire[2..].iter_mut().zip(args) {
        *dest = !src;
    }
    wire
}

pub fn mchose_reply(wire: &[u8], report: u8, command: u8) -> Result<Vec<u8>> {
    let length = if report == 0x11 { 21 } else { 65 };
    if wire.len() != length || wire[0] != report || wire[1] != !command {
        return Err("迈从回复长度或命令不匹配，设备可能休眠或被官方驱动占用".into());
    }
    Ok(wire[2..].iter().map(|v| !v).collect())
}

pub fn mchose_battery(body: &[u8]) -> Result<(u8, Option<bool>, bool)> {
    // Official Pat schema: vid u16, pid u16, firmware u32, flags u8,
    // battery u8, charge u8. This is NOT the A5 Pro Max protocol.
    if body.len() < 11 || u16::from_le_bytes([body[2], body[3]]) == 0 {
        return Err("迈从接收器未返回在线鼠标身份".into());
    }
    if body[9] > 100 {
        return Err("迈从返回的电量超出 0–100，已拒绝显示".into());
    }
    let charging = match body[10] {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    // The official little-endian bit parser puts connectMode in bits 0..2.
    // A5 Ultra live fixture: flags 0x09 => wireless (1), online (bit 3).
    Ok((body[9], charging, body[8] & 7 == 0))
}

pub const MCHOSE_RATES: [u16; 6] = [125, 500, 1000, 2000, 4000, 8000];

#[derive(Clone, Debug)]
pub struct MchoseConfig {
    pub raw: Vec<u8>,
    pub wireless_stage: usize,
    pub wired_stage: usize,
    pub wireless_rate: usize,
    pub wired_rate: usize,
    pub stages: usize,
}

impl MchoseConfig {
    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() < 63 {
            return Err("迈从配置报告过短".into());
        }
        let config = Self {
            raw: body.to_vec(),
            wireless_stage: (body[2] & 15) as usize,
            wired_stage: (body[1] & 15) as usize,
            wireless_rate: (body[2] >> 4) as usize,
            wired_rate: (body[1] >> 4) as usize,
            stages: body[16] as usize,
        };
        if !(1..=6).contains(&config.stages)
            || config.wireless_stage >= config.stages
            || config.wired_stage >= config.stages
            || config.wireless_rate >= 6
            || config.wired_rate >= 6
        {
            return Err("迈从配置字段不在已知范围内，停止写入".into());
        }
        Ok(config)
    }

    pub fn dpi(&self, wired: bool) -> u16 {
        let stage = if wired {
            self.wired_stage
        } else {
            self.wireless_stage
        };
        u16::from_le_bytes([self.raw[4 + stage * 2], self.raw[5 + stage * 2]])
    }

    pub fn dpi_arguments(&self, value: u16, wired: bool) -> Vec<u8> {
        // Official 0x40 schema, preserving all other stages and split-Y mode.
        let stage = if wired {
            self.wired_stage
        } else {
            self.wireless_stage
        };
        let mut args = vec![self.wired_stage as u8, self.wireless_stage as u8, 0];
        let mut x = self.raw[4..16].to_vec();
        x[stage * 2..stage * 2 + 2].copy_from_slice(&value.to_le_bytes());
        args.extend(x);
        args.push(self.stages as u8);
        args.push(self.raw[50]);
        let mut y = self.raw[51..63].to_vec();
        y[stage * 2..stage * 2 + 2].copy_from_slice(&value.to_le_bytes());
        args.extend(y);
        args
    }
}

/// 0 means leave untouched for M HUB processing flags; 1 on, 2 off.
pub fn mchose_tuning_arguments(
    config: &MchoseConfig,
    key: crate::model::Tuning,
    value: u16,
) -> Result<Vec<u8>> {
    use crate::model::Tuning;
    let lod = config.raw[17] & 3;
    if !(1..=2).contains(&lod) {
        return Err("传感器 LOD 字段未知，停止写入".into());
    }
    let mut args = vec![lod, 0, 0, 0, 0, 0, 0, 0, 0];
    match key {
        Tuning::Lod if (1..=2).contains(&value) => args[0] = value as u8,
        Tuning::Ripple | Tuning::Angle | Tuning::Motion if value <= 1 => {
            let offset = match key {
                Tuning::Ripple => 1,
                Tuning::Angle => 2,
                _ => 3,
            };
            args[offset] = if value == 1 { 1 } else { 2 };
        }
        _ => return Err("传感器参数不在已知范围内".into()),
    }
    Ok(args)
}

/// Preserve the device's stage numbering, Y DPI and reserved bytes.
pub fn razer_stage_table(body: &[u8]) -> Result<(Vec<u16>, u8)> {
    if body.len() != 38 || !(1..=5).contains(&body[2]) || !(1..=body[2]).contains(&body[1]) {
        return Err("雷蛇 DPI 档位结构未知".into());
    }
    let mut stages = Vec::new();
    for i in 0..body[2] as usize {
        let at = 3 + i * 7;
        let dpi = u16::from_be_bytes([body[at + 1], body[at + 2]]);
        if !(100..=30000).contains(&dpi) {
            return Err("雷蛇 DPI 档位值异常".into());
        }
        stages.push(dpi);
    }
    Ok((stages, body[1] - 1))
}
pub fn razer_stage_arguments(body: &[u8], stage: u8, dpi: Option<u16>) -> Result<Vec<u8>> {
    let (stages, _) = razer_stage_table(body)?;
    if stage as usize >= stages.len() {
        return Err("DPI 档位越界".into());
    }
    let mut out = body.to_vec();
    out[0] = 1;
    out[1] = stage + 1;
    if let Some(v) = dpi {
        if !(100..=30000).contains(&v) || v % 50 != 0 {
            return Err("DPI 值越界".into());
        }
        let at = 3 + stage as usize * 7;
        out[at + 1..at + 3].copy_from_slice(&v.to_be_bytes());
        out[at + 3..at + 5].copy_from_slice(&v.to_be_bytes());
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_write_changes_only_requested_toggle() {
        let mut raw = vec![0; 63];
        raw[16] = 1;
        raw[17] = 0x95;
        let c = MchoseConfig::decode(&raw).unwrap();
        assert_eq!(
            mchose_tuning_arguments(&c, crate::model::Tuning::Angle, 0).unwrap(),
            vec![1, 0, 2, 0, 0, 0, 0, 0, 0]
        );
        assert!(mchose_tuning_arguments(&c, crate::model::Tuning::Lod, 3).is_err());
    }
    #[test]
    fn razer_stage_edit_preserves_other_slots_and_reserved_bytes() {
        let mut table = vec![0; 38];
        table[1] = 2;
        table[2] = 2;
        table[3..10].copy_from_slice(&[1, 1, 144, 1, 144, 7, 9]);
        table[10..17].copy_from_slice(&[2, 3, 32, 3, 32, 6, 8]);
        let mut edited = razer_stage_arguments(&table, 1, Some(1200)).unwrap();
        assert_eq!(&edited[11..15], &[4, 176, 4, 176]);
        edited[0] = 0;
        edited[11..15].copy_from_slice(&table[11..15]);
        assert_eq!(edited, table);
        assert!(razer_stage_arguments(&table, 2, None).is_err());
    }
    fn response(req: &[u8; 91]) -> [u8; 91] {
        let mut r = *req;
        r[1] = 2;
        r[10] = 233;
        r[89] = r[3..89].iter().fold(0, |a, b| a ^ b);
        r
    }

    #[test]
    fn razer_live_battery_fixture_and_corruption() {
        let req = razer_request(7, 0x80, 2, &[]);
        assert_eq!(&req[1..9], &[0, 0x1f, 0, 0, 0, 2, 7, 0x80]);
        assert_eq!(req[89], 0x85);
        let mut r = response(&req);
        assert_eq!(razer_percent(razer_reply(&r, &req).unwrap()[1]), 91);
        r[10] ^= 1;
        assert!(razer_reply(&r, &req).is_err());
    }

    #[test]
    fn rejects_stale_busy_and_short_razer_responses() {
        let req = razer_request(7, 0x80, 2, &[]);
        let mut r = response(&req);
        r[1] = 1;
        assert!(razer_reply(&r, &req).is_err());
        let other = razer_request(7, 0x84, 2, &[]);
        assert!(razer_reply(&response(&other), &req).is_err());
        assert!(razer_reply(&r[..90], &req).is_err());
        assert_eq!(razer_percent(0), 0);
        assert_eq!(razer_percent(255), 100);
    }

    #[test]
    fn mchose_official_schema_no_ten_percent_guessing() {
        let mut b = [0u8; 19];
        b[2] = 1;
        b[8] = 0x09;
        b[9] = 8;
        b[10] = 1;
        assert_eq!(mchose_battery(&b).unwrap(), (8, Some(true), false));
        b[9] = 101;
        assert!(mchose_battery(&b).is_err());
        b[9] = 0;
        assert_eq!(mchose_battery(&b).unwrap().0, 0);
        b[2] = 0;
        assert!(mchose_battery(&b).is_err());
    }

    #[test]
    fn mchose_frame_and_unrelated_response_rejected() {
        let wire = mchose_request(0x11, 6, &[]);
        assert_eq!(&wire[..4], &[0x11, 0xf9, 0xff, 0xff]);
        assert_eq!(wire.len(), 21);
        assert!(mchose_reply(&wire, 0x11, 3).is_err());
        assert_eq!(mchose_reply(&wire, 0x11, 6).unwrap().len(), 19);
    }

    #[test]
    fn dpi_edit_preserves_other_stages_and_y_mode() {
        let mut raw = vec![0; 63];
        raw[1] = 0x20;
        raw[2] = 0x21;
        raw[16] = 3;
        raw[50] = 0xff;
        for i in 0..6 {
            raw[4 + i * 2..6 + i * 2].copy_from_slice(&(400u16 * (i as u16 + 1)).to_le_bytes());
            raw[51 + i * 2..53 + i * 2].copy_from_slice(&(400u16 * (i as u16 + 1)).to_le_bytes());
        }
        let cfg = MchoseConfig::decode(&raw).unwrap();
        assert_eq!(cfg.dpi(false), 800);
        let args = cfg.dpi_arguments(1600, false);
        assert_eq!(&args[3..5], &400u16.to_le_bytes());
        assert_eq!(&args[5..7], &1600u16.to_le_bytes());
        assert_eq!(&args[7..9], &1200u16.to_le_bytes());
        assert_eq!(args[16], 0xff);
        assert_eq!(&args[19..21], &1600u16.to_le_bytes());
    }
}
