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
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("灯效配置格式错误：{e}"))?;
    let config = local_config(&document)?;
    validate(&config)?;
    Ok(Some(config))
}
fn local_config(document: &serde_json::Value) -> Result<LightingConfig> {
    let object = document.as_object().ok_or("灯效配置必须是 JSON 对象")?;
    // Keep strict request validation while accepting newer fields in local files.
    let known = ["enabled", "mode", "rgb", "speed"]
        .into_iter()
        .filter_map(|key| object.get(key).map(|value| (key.into(), value.clone())))
        .collect();
    serde_json::from_value(serde_json::Value::Object(known))
        .map_err(|e| format!("灯效配置格式错误：{e}"))
}
fn save_config(config: &LightingConfig) -> Result<()> {
    crate::storage::write_json_preserving(
        &config_path(),
        serde_json::to_value(config).map_err(|e| e.to_string())?,
    )
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
fn state(enabled: bool, saved: Result<Option<LightingConfig>>) -> LightingState {
    let (saved, warning) = match saved {
        Ok(saved) => (saved, String::new()),
        Err(error) => (
            None,
            format!(" 本地灯效配置不可用，已保留原文件；不影响开关控制：{error}"),
        ),
    };
    LightingState { enabled, saved, notice: format!("灯光开关已从设备读取；模式、颜色和速度显示本工具保存的配置，设备不提供已确认的完整灯效读回。{warning}") }
}
pub fn read() -> Result<LightingState> {
    // A corrupt/unsupported local palette must not hide a readable hardware switch.
    let enabled = LightingDevice::open()?.enabled()?;
    Ok(state(enabled, saved_config()))
}
fn switch_transaction(
    enabled: bool,
    read: impl Fn() -> Result<bool>,
    write: impl Fn(bool) -> Result<()>,
) -> Result<()> {
    let original = read()?;
    let changed = write(enabled).and_then(|_| {
        if read()? == enabled {
            Ok(())
        } else {
            Err("G2 Pro 灯光开关读回不一致".into())
        }
    });
    if let Err(error) = changed {
        let restored = write(original).and_then(|_| {
            if read()? == original {
                Ok(())
            } else {
                Err("原开关状态读回不一致".into())
            }
        });
        return Err(match restored {
            Ok(()) => format!("{error}；已恢复原灯光开关，模式和颜色未改动"),
            Err(restore) => format!("{error}；原灯光开关恢复失败：{restore}"),
        });
    }
    Ok(())
}
/// The UI's immediate switch must never send FF mode/palette commands.
pub fn set_enabled(enabled: bool) -> Result<LightingState> {
    let device = LightingDevice::open()?;
    switch_transaction(
        enabled,
        || device.enabled(),
        |value| device.set_enabled(value),
    )?;
    let mut result = state(enabled, saved_config());
    if let Some(previous) = result.saved.as_ref() {
        let mut updated = previous.clone();
        updated.enabled = enabled;
        match save_config(&updated) {
            Ok(()) => result.saved = Some(updated),
            Err(error) => result.notice.push_str(&format!(
                " 灯光开关已生效并读回，但本地配置保存失败；仍显示此前保存的配置：{error}"
            )),
        }
    }
    result.notice.insert_str(
        0,
        "此次仅写入并读回开关，没有发送模式、颜色或速度；设备原灯效保持不变。 ",
    );
    Ok(result)
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
    save_config(config).map_err(|e| format!("灯效已发送且开关已确认，但本地配置保存失败：{e}"))?;
    Ok(state(config.enabled, Ok(Some(config.clone()))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switch_only_reads_writes_and_confirms_the_boolean() {
        let value = std::cell::Cell::new(true);
        let writes = std::cell::RefCell::new(Vec::new());
        switch_transaction(
            false,
            || Ok(value.get()),
            |enabled| {
                writes.borrow_mut().push(enabled);
                value.set(enabled);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*writes.borrow(), [false]);
        assert!(!value.get());
    }
    #[test]
    fn switch_failure_restores_original_and_reports_failed_restoration() {
        let unknown = switch_transaction(
            true,
            || Err("原开关未知".into()),
            |_| panic!("No write is allowed when the original switch cannot be read"),
        );
        assert_eq!(unknown.unwrap_err(), "原开关未知");
        for fail_restore in [false, true] {
            let value = std::cell::Cell::new(true);
            let reads = std::cell::Cell::new(0);
            let writes = std::cell::RefCell::new(Vec::new());
            let error = switch_transaction(
                false,
                || {
                    reads.set(reads.get() + 1);
                    if reads.get() == 2 {
                        Err("测试读取失败".into())
                    } else {
                        Ok(value.get())
                    }
                },
                |enabled| {
                    writes.borrow_mut().push(enabled);
                    if enabled && fail_restore {
                        return Err("测试恢复写入失败".into());
                    }
                    value.set(enabled);
                    Ok(())
                },
            )
            .unwrap_err();
            assert_eq!(*writes.borrow(), [false, true]);
            if fail_restore {
                assert!(error.contains("原灯光开关恢复失败"));
                assert!(!value.get());
            } else {
                assert!(error.contains("已恢复原灯光开关"));
                assert!(value.get());
            }
        }
    }
    #[test]
    fn local_switch_preserves_newer_fields_without_relaxing_requests() {
        let mut document = serde_json::json!({"enabled":true,"mode":"Breathing","rgb":[1,2,3],"speed":4,"future":{"opaque":7}});
        let mut config = local_config(&document).unwrap();
        assert!(serde_json::from_value::<LightingConfig>(document.clone()).is_err());
        config.enabled = false;
        crate::storage::merge_json(&mut document, serde_json::to_value(&config).unwrap());
        assert_eq!(document["future"]["opaque"], 7);
        assert_eq!(document["mode"], "Breathing");
        assert_eq!(document["rgb"], serde_json::json!([1, 2, 3]));
        assert_eq!(document["speed"], 4);
        assert_eq!(document["enabled"], false);
    }
    #[test]
    fn corrupt_saved_palette_does_not_hide_the_hardware_switch() {
        for enabled in [false, true] {
            let result = state(enabled, Err("无效 JSON".into()));
            assert_eq!(result.enabled, enabled);
            assert!(result.saved.is_none());
            assert!(result.notice.contains("不影响开关控制"));
            assert!(result.notice.contains("无效 JSON"));
        }
        assert!(state(false, Ok(None)).saved.is_none());
    }
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
