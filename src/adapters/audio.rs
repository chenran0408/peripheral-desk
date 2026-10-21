//! Windows endpoint controls; no vendor SDK or background process is used.
use crate::{
    backend::now,
    model::{Action, State, Tuning},
    protocol::Result,
};
use std::{collections::BTreeMap, ptr::null};
use windows::Win32::{
    Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
    Media::Audio::{Endpoints::IAudioEndpointVolume, *},
    System::Com::{
        StructuredStorage::{
            PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32WithDefault,
        },
        *,
    },
};
use windows_sys::Win32::System::Registry::*;

// This value lives in the endpoint's FxProperties store on Windows. Reading
// only the ordinary IPropertyStore can miss it; the vendor SDK otherwise
// enables system effects as a side effect of reading a generic parameter.
fn registry_fx_disabled(device: &IMMDevice, capture: bool) -> Result<bool> {
    unsafe {
        let id = device.GetId().map_err(error)?;
        let text = id.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(id.0 as _));
        let text = text?;
        let endpoint = text.rsplit('.').next().ok_or("音频端点标识无效")?;
        let path = crate::hid::wide(&format!(
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\{}\\{}\\FxProperties",
            if capture { "Capture" } else { "Render" },
            endpoint
        ));
        let name = crate::hid::wide("{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5");
        let mut value = 0u32;
        let mut size = 4u32;
        let status = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        );
        match status {
            0 => Ok(value == 1),
            2 | 3 => Ok(false),
            code => Err(format!("无法读取 Windows 音频增强状态（{code}）")),
        }
    }
}
struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub struct AudioSession {
    volumes: BTreeMap<Tuning, IAudioEndpointVolume>,
    fx_disabled: bool,
    _com: ComApartment,
}
fn error(e: windows::core::Error) -> String {
    format!("Windows 音频接口：{e}")
}
impl AudioSession {
    pub fn open() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(error)?;
            let com = ComApartment;
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(error)?;
            let mut volumes = BTreeMap::new();
            let mut fx_disabled = false;
            for flow in [eRender, eCapture] {
                let devices = enumerator
                    .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
                    .map_err(error)?;
                for i in 0..devices.GetCount().map_err(error)? {
                    let device = devices.Item(i).map_err(error)?;
                    let store = device.OpenPropertyStore(STGM_READ).map_err(error)?;
                    let mut value = store.GetValue(&PKEY_Device_FriendlyName).map_err(error)?;
                    let name = PropVariantToStringAlloc(&value);
                    let cleared = PropVariantClear(&mut value);
                    let name = name.map_err(error)?;
                    let text = name.to_string().map_err(|e| e.to_string());
                    CoTaskMemFree(Some(name.0 as _));
                    cleared.map_err(error)?;
                    let text = text?.to_uppercase();
                    let Some(key) = endpoint_key(&text, flow == eCapture) else {
                        continue;
                    };
                    fx_disabled |= registry_fx_disabled(&device, flow == eCapture)?;
                    if let Ok(mut value) = store.GetValue(&PKEY_AudioEndpoint_Disable_SysFx) {
                        fx_disabled |= PropVariantToUInt32WithDefault(&value, 0) == 1;
                        PropVariantClear(&mut value).map_err(error)?;
                    }
                    let volume: IAudioEndpointVolume =
                        device.Activate(CLSCTX_ALL, None).map_err(error)?;
                    if volumes.insert(key, volume).is_some() {
                        return Err("检测到重复 G2 Pro 音频端点，无法确定目标设备".into());
                    }
                }
            }
            if !volumes.contains_key(&Tuning::GameVolume) {
                return Err("G2 Pro 播放端点未启用或名称未识别，请检查 Windows 声音设置".into());
            }
            Ok(Self {
                volumes,
                fx_disabled,
                _com: com,
            })
        }
    }
}
pub fn effects_available() -> Result<()> {
    let session = AudioSession::open()?;
    if session.fx_disabled {
        Err("Windows 已关闭音频增强，音效暂不可用；音量控制不受影响".into())
    } else {
        Ok(())
    }
}
impl super::DeviceSession for AudioSession {
    fn read(&self) -> Result<State> {
        unsafe {
            let mut state = State {
                present: true,
                fresh: true,
                wired: true,
                updated: now(),
                identity: "35BB:A100 · USB Audio".into(),
                ..State::default()
            };
            for (key, volume) in &self.volumes {
                let value = volume.GetMasterVolumeLevelScalar().map_err(error)?;
                state
                    .tuning
                    .insert(*key, (value * 100.).round().clamp(0., 100.) as u16);
                if *key == Tuning::MicVolume {
                    state.tuning.insert(
                        Tuning::MicMute,
                        u16::from(volume.GetMute().map_err(error)?.as_bool()),
                    );
                }
            }
            match if self.fx_disabled {
                Err("Windows 已关闭音频增强，音效暂不可用；音量控制不受影响".into())
            } else {
                crate::audio_fx::query(None)
            } {
                Ok(reply) => state.tuning.extend(reply.values),
                Err(e) => state.settings_error = e,
            }
            Ok(state)
        }
    }
    fn apply(&self, action: Action) -> Result<()> {
        unsafe {
            let Action::Tune(key, value) = action else {
                return Err("耳机不支持该设置".into());
            };
            if crate::audio_fx::spec(key).is_some() {
                if self.fx_disabled {
                    return Err("Windows 已关闭音频增强，请先在声音设置中启用".into());
                }
                crate::audio_fx::query(Some((key, value)))?;
                return Ok(());
            }
            if key == Tuning::MicMute {
                if value > 1 {
                    return Err("静音值越界".into());
                }
                let endpoint = self
                    .volumes
                    .get(&Tuning::MicVolume)
                    .ok_or("麦克风端点未启用")?;
                endpoint.SetMute(value == 1, null()).map_err(error)?;
                if endpoint.GetMute().map_err(error)?.as_bool() != (value == 1) {
                    return Err("麦克风静音读回不匹配".into());
                }
            } else {
                if value > 100 {
                    return Err("音量应在 0–100 之间".into());
                }
                let endpoint = self.volumes.get(&key).ok_or("音频端点未启用")?;
                endpoint
                    .SetMasterVolumeLevelScalar(value as f32 / 100., null())
                    .map_err(error)?;
                let check = endpoint.GetMasterVolumeLevelScalar().map_err(error)?;
                if (check * 100. - value as f32).abs() > 0.6 {
                    return Err("音量读回不匹配".into());
                }
            }
            Ok(())
        }
    }
}
fn endpoint_key(name: &str, capture: bool) -> Option<Tuning> {
    if !name.contains("HECATE G2 PRO") {
        return None;
    }
    if capture {
        Some(Tuning::MicVolume)
    } else if name.contains("GAMING") {
        Some(Tuning::GameVolume)
    } else if name.contains("CHAT") {
        Some(Tuning::ChatVolume)
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::DeviceSession;
    #[test]
    fn endpoint_matching_excludes_other_models() {
        assert_eq!(
            endpoint_key("HECATE GAMING HEADSET (HECATE G2 PRO)", false),
            Some(Tuning::GameVolume)
        );
        assert_eq!(
            endpoint_key("HECATE CHAT HEADSET (HECATE G2 PRO)", false),
            Some(Tuning::ChatVolume)
        );
        assert_eq!(endpoint_key("MIC (HECATE G4 PRO)", true), None);
        assert_eq!(
            endpoint_key("HECATE GAMING HEADSET (HECATE G2 PRO)", true),
            Some(Tuning::MicVolume)
        );
    }
    #[test]
    #[ignore = "Requires physical G2 Pro; briefly lowers output levels then restores exact scalars"]
    fn live_audio_roundtrip_restores_exact_levels() {
        unsafe {
            let session = AudioSession::open().unwrap();
            let before = session.read().unwrap();
            assert!(before.tuning.contains_key(&Tuning::GameVolume));
            for (key, endpoint) in &session.volumes {
                let original = endpoint.GetMasterVolumeLevelScalar().unwrap();
                let changed = (original - 0.01).max(0.);
                let result = endpoint
                    .SetMasterVolumeLevelScalar(changed, null())
                    .and_then(|_| endpoint.GetMasterVolumeLevelScalar());
                let restore = endpoint.SetMasterVolumeLevelScalar(original, null());
                println!(
                    "{:?}: {:.3} -> {:?}; restore {:?}",
                    key, original, result, restore
                );
                assert!(restore.is_ok());
                assert!((result.unwrap() - changed).abs() < 0.006);
                assert!((endpoint.GetMasterVolumeLevelScalar().unwrap() - original).abs() < 0.006);
            }
            if let Some(endpoint) = session.volumes.get(&Tuning::MicVolume) {
                let muted = endpoint.GetMute().unwrap().as_bool();
                endpoint.SetMute(muted, null()).unwrap();
                assert_eq!(endpoint.GetMute().unwrap().as_bool(), muted);
            }
        }
    }
}
