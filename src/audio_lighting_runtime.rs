use super::*;
use crate::{hid, protocol::Result};
use std::{
    ptr::{null, null_mut},
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Devices::HumanInterfaceDevice::{HidD_GetFeature, HidD_SetFeature},
    Foundation::*,
    Storage::FileSystem::*,
};

fn config_path() -> std::path::PathBuf {
    crate::backend::data_path().with_file_name("g2-pro-lighting.json")
}
fn validate(config: &LightingConfig) -> Result<()> {
    if !(1..=10).contains(&config.speed) {
        return Err("灯效速度应在 1–10 之间".into());
    }
    Ok(())
}
fn saved_config() -> Result<Option<LightingConfig>> {
    let bytes = match std::fs::read(config_path()) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取已保存灯效失败：{e}")),
    };
    let config: LightingConfig =
        serde_json::from_slice(&bytes).map_err(|e| format!("灯效配置格式错误：{e}"))?;
    validate(&config)?;
    Ok(Some(config))
}
struct LightingDevice(HANDLE);
impl Drop for LightingDevice {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl LightingDevice {
    fn open() -> Result<Self> {
        let all = hid::enumerate()?;
        let targets: Vec<_> = all
            .iter()
            .filter(|i| {
                i.vid == 0x35bb && i.pid == 0xa100 && i.feature_len == 16 && i.usage_page == 12
            })
            .collect();
        if targets.len() != 1 {
            return Err("需要唯一的 G2 Pro 灯光 HID 接口（35BB:A100）".into());
        }
        unsafe {
            let handle = CreateFileW(
                hid::wide(&targets[0].path).as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "打开 G2 Pro 灯光接口失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self(handle))
        }
    }
    fn output(&self, bytes: &[u8; 16]) -> Result<()> {
        let mut written = 0;
        unsafe {
            // HECATE uses hid_write, whose Windows transport is WriteFile.
            if WriteFile(self.0, bytes.as_ptr(), 16, &mut written, null_mut()) == 0 || written != 16
            {
                return Err(format!(
                    "发送 G2 Pro 灯效失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }
    fn enabled(&self) -> Result<bool> {
        let mut request = [0; 16];
        request[..2].copy_from_slice(&[0xed, 0x0b]);
        let mut last = String::new();
        // The first feature can still contain an earlier selector's response.
        // Retry only this read transaction, keeping the official G2 Pro header
        // and boolean checks intact; never repeat a setting write here.
        for delay in [25, 50, 100] {
            self.output(&request)?;
            thread::sleep(Duration::from_millis(delay));
            let mut reply = request;
            if !unsafe { HidD_GetFeature(self.0, reply.as_mut_ptr().cast(), 16) } {
                last = format!(
                    "读取 G2 Pro 灯光开关失败：{}",
                    std::io::Error::last_os_error()
                );
                continue;
            }
            match enabled_reply(&reply) {
                Ok(enabled) => return Ok(enabled),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut select = [0; 16];
        select[..2].copy_from_slice(&[0xed, 0x0b]);
        self.output(&select)?;
        // HECATE supplies four logical bytes to hid_send_feature_report, whose
        // Windows backend pads to FeatureReportByteLength before SetFeature.
        // Calling Win32 directly requires the same 16-byte zero-padded report.
        // HIDAPI 852cc68b8e0e4c7eba0815ef992951cd8b75c55b/windows/hid.c.
        let mut value = [0; 16];
        value[..4].copy_from_slice(&[0xed, 2, 0x0b, u8::from(enabled)]);
        unsafe {
            if !HidD_SetFeature(self.0, value.as_ptr().cast(), value.len() as u32) {
                return Err(format!(
                    "设置 G2 Pro 灯光开关失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }
    fn program(&self, config: &LightingConfig) -> Result<()> {
        self.set_enabled(config.enabled)?;
        if config.enabled {
            for packet in packets(config) {
                self.output(&packet)?;
            }
        }
        if self.enabled()? != config.enabled {
            return Err("G2 Pro 灯光开关读回不一致".into());
        }
        Ok(())
    }
}
fn enabled_reply(reply: &[u8; 16]) -> Result<bool> {
    if reply[..3] != [0xed, 2, 0x0b] || reply[3] > 1 {
        return Err(format!(
            "G2 Pro 灯光开关回复格式不匹配（{:02X?}）",
            &reply[..4]
        ));
    }
    Ok(reply[3] == 1)
}
fn packets(config: &LightingConfig) -> Vec<[u8; 16]> {
    let mut stop = [0; 16];
    stop[..2].copy_from_slice(&[0xff, 2]);
    let mut setup = stop;
    setup[1] = 3;
    setup[2] = match config.mode {
        LightingMode::Static => 0,
        LightingMode::Breathing => 2,
        LightingMode::Rainbow => 1,
    };
    setup[3..5].copy_from_slice(&128u16.to_le_bytes());
    setup[5..7].copy_from_slice(&(10u16 * (11 - config.speed as u16)).to_le_bytes());
    let count = if config.mode == LightingMode::Static {
        1
    } else {
        128
    };
    let colors: Vec<_> = (0..count)
        .map(|i| match config.mode {
            LightingMode::Static => config.rgb,
            LightingMode::Breathing => config
                .rgb
                .map(|v| ((v as f32) * (1.0 - i as f32 / 128.0)) as u8),
            // The G2 accepts an arbitrary 128-color palette. Generate a smooth
            // spectrum ourselves rather than copying the vendor's color table.
            LightingMode::Rainbow => {
                let phase = i * 6;
                let sector = phase / 128;
                let rising = (phase % 128 * 200 / 128) as u8;
                let falling = 200 - rising;
                match sector {
                    0 => [200, rising, 0],
                    1 => [falling, 200, 0],
                    2 => [0, 200, rising],
                    3 => [0, falling, 200],
                    4 => [rising, 0, 200],
                    _ => [200, 0, falling],
                }
            }
        })
        .collect();
    let mut result = vec![stop, setup];
    for (page, chunk) in colors.chunks(4).enumerate() {
        let mut packet = [0; 16];
        packet[..4].copy_from_slice(&[0xff, 4, (page * 4) as u8, 0]);
        for (slot, color) in chunk.iter().enumerate() {
            packet[4 + slot * 3..7 + slot * 3].copy_from_slice(color);
        }
        result.push(packet);
    }
    let mut start = [0; 16];
    start[..2].copy_from_slice(&[0xff, 1]);
    result.push(start);
    start[1] = 5;
    result.push(start);
    result
}
fn state(enabled: bool, saved: Option<LightingConfig>) -> LightingState {
    LightingState { enabled, saved, notice: "灯光开关已从设备读取；模式、颜色和速度显示本工具保存的配置，设备不提供已确认的完整灯效读回。".into() }
}
pub fn read() -> Result<LightingState> {
    let saved = saved_config()?;
    Ok(state(LightingDevice::open()?.enabled()?, saved))
}
pub fn apply(config: &LightingConfig) -> Result<LightingState> {
    validate(config)?;
    let previous = saved_config()?;
    let device = LightingDevice::open()?;
    let original_enabled = device.enabled()?;
    if let Err(error) = device.program(config) {
        let recovery = if let Some(mut previous) = previous {
            previous.enabled = original_enabled;
            device.program(&previous)
        } else {
            device.set_enabled(original_enabled).and_then(|_| {
                if device.enabled()? == original_enabled {
                    Ok(())
                } else {
                    Err("原开关状态读回不一致".into())
                }
            })
        };
        return Err(match recovery {
            Ok(()) => format!(
                "{error}；已恢复原开关，并尽可能恢复本工具之前保存的灯效；原设备颜色无法完整读取"
            ),
            Err(restore) => format!("{error}；灯效恢复失败：{restore}"),
        });
    }
    crate::storage::atomic_write(
        &config_path(),
        &serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("灯效已发送且开关已确认，但本地配置保存失败：{e}"))?;
    Ok(state(config.enabled, Some(config.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn g2_enabled_reply_keeps_official_header_and_boolean_checks() {
        let mut reply = [0; 16];
        reply[..4].copy_from_slice(&[0xed, 2, 0x0b, 0]);
        assert!(!enabled_reply(&reply).unwrap());
        reply[3] = 1;
        assert!(enabled_reply(&reply).unwrap());
        reply[3] = 2;
        assert!(enabled_reply(&reply).is_err());
        reply[..4].copy_from_slice(&[0xed, 2, 0x0a, 1]);
        assert!(enabled_reply(&reply).is_err());
    }
    #[test]
    fn g2_lighting_packet_layout_matches_its_dedicated_endpoint() {
        let mut config = LightingConfig {
            rgb: [4, 193, 255],
            ..LightingConfig::default()
        };
        let static_packets = packets(&config);
        assert_eq!(&static_packets[1][..7], &[255, 3, 0, 128, 0, 60, 0]);
        assert_eq!(&static_packets[2][..7], &[255, 4, 0, 0, 4, 193, 255]);
        assert_eq!(static_packets.len(), 5);
        config.mode = LightingMode::Breathing;
        let breathing = packets(&config);
        assert_eq!(breathing.len(), 36);
        assert_eq!(breathing[1][2], 2);
        assert_eq!(breathing[33][2], 124);
        assert_eq!(&breathing[2][4..7], &config.rgb);
        config.speed = 0;
        assert!(validate(&config).is_err());
    }
}
