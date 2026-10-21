//! Isolated bridge to the user's existing C-Media SDK. No vendor binaries are shipped.
use crate::{model::Tuning, protocol::Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::*,
};
pub struct FxSpec {
    pub key: Tuning,
    pub channel: usize,
    pub property: &'static str,
    pub max: u16,
    pub offset: i32,
}
pub const SPECS: &[FxSpec] = &[
    FxSpec {
        key: Tuning::EqEnabled,
        channel: 0,
        property: "Enable_EQ_GFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::Eq30,
        channel: 0,
        property: "EQ_Slider1",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq60,
        channel: 0,
        property: "EQ_Slider2",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq120,
        channel: 0,
        property: "EQ_Slider3",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq250,
        channel: 0,
        property: "EQ_Slider4",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq500,
        channel: 0,
        property: "EQ_Slider5",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq1k,
        channel: 0,
        property: "EQ_Slider6",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq2k,
        channel: 0,
        property: "EQ_Slider7",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq4k,
        channel: 0,
        property: "EQ_Slider8",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq8k,
        channel: 0,
        property: "EQ_Slider9",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::Eq16k,
        channel: 0,
        property: "EQ_Slider10",
        max: 40,
        offset: 20,
    },
    FxSpec {
        key: Tuning::SurroundEnabled,
        channel: 0,
        property: "Enable_CONLAB_HPSPK_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SurroundRoom,
        channel: 0,
        property: "CONLAB_HPSPK_ROOMSIZE",
        max: 2,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SmartVolumeEnabled,
        channel: 0,
        property: "Enable_ADAPTIVEVOLUME_GFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::SmartVolumeLevel,
        channel: 0,
        property: "ADAPTIVEVOLUME_LEVEL",
        max: 100,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BrillianceEnabled,
        channel: 0,
        property: "Enable_AUDIOBRILLIANT_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BrillianceLevel,
        channel: 0,
        property: "AUDIOBRILLIANT_LEVEL",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BassEnabled,
        channel: 0,
        property: "Enable_VIRTUALBASS_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::BassLevel,
        channel: 0,
        property: "VIRTUALBASS_Level",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ClarityEnabled,
        channel: 0,
        property: "Enable_VOICECLARITY_LFX",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ClarityLevel,
        channel: 0,
        property: "VOICECLARITY_LEVEL",
        max: 10,
        offset: 0,
    },
    FxSpec {
        key: Tuning::MicNoise,
        channel: 2,
        property: "AI_NR_Enable",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::ChatNoise,
        channel: 1,
        property: "AI_Playback_NR_Enable",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::VoiceEnabled,
        channel: 2,
        property: "Enable_MAGICVOICE",
        max: 1,
        offset: 0,
    },
    FxSpec {
        key: Tuning::VoiceMode,
        channel: 2,
        property: "MagicVoice_Selection",
        max: 3,
        offset: 0,
    },
];
pub fn spec(key: Tuning) -> Option<&'static FxSpec> {
    SPECS.iter().find(|s| s.key == key)
}
pub fn page(key: Tuning) -> u8 {
    match key {
        Tuning::GameVolume | Tuning::ChatVolume | Tuning::MicVolume | Tuning::MicMute => 0,
        Tuning::EqEnabled
        | Tuning::Eq30
        | Tuning::Eq60
        | Tuning::Eq120
        | Tuning::Eq250
        | Tuning::Eq500
        | Tuning::Eq1k
        | Tuning::Eq2k
        | Tuning::Eq4k
        | Tuning::Eq8k
        | Tuning::Eq16k => 1,
        Tuning::SurroundEnabled
        | Tuning::SurroundRoom
        | Tuning::SmartVolumeEnabled
        | Tuning::SmartVolumeLevel => 2,
        Tuning::BrillianceEnabled
        | Tuning::BrillianceLevel
        | Tuning::BassEnabled
        | Tuning::BassLevel
        | Tuning::ClarityEnabled
        | Tuning::ClarityLevel => 3,
        _ => 4,
    }
}
pub const EQ_BANDS: &[(Tuning, &str)] = &[
    (Tuning::Eq30, "30"),
    (Tuning::Eq60, "60"),
    (Tuning::Eq120, "120"),
    (Tuning::Eq250, "250"),
    (Tuning::Eq500, "500"),
    (Tuning::Eq1k, "1k"),
    (Tuning::Eq2k, "2k"),
    (Tuning::Eq4k, "4k"),
    (Tuning::Eq8k, "8k"),
    (Tuning::Eq16k, "16k"),
];
pub const PAGES: &[&str] = &["音量", "均衡器", "环绕", "增强", "语音"];
#[derive(Serialize, Deserialize)]
struct Config {
    path: PathBuf,
}
fn config_path() -> PathBuf {
    crate::backend::data_path().with_file_name("hecate-sdk.json")
}
pub fn configure(path: &Path) -> Result<()> {
    validate_library(path)?;
    let bytes = serde_json::to_vec_pretty(&Config {
        path: path.to_path_buf(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(config_path().parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(config_path(), bytes).map_err(|e| e.to_string())
}
fn validate_library(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || !path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("osConfLib.dll"))
    {
        return Err("请选择官方安装目录中的 x64/osConfLib.dll".into());
    }
    let data = std::fs::read(path).map_err(|e| format!("无法读取音效组件：{e}"))?;
    let hash = format!("{:x}", Sha256::digest(&data));
    if hash != "576c1306133a33b975d7ecf25a2b7078052b2566e3705715c247d1e1a1ef2947" {
        return Err("该音效组件版本尚未验证，请使用已验证的 C-Media 1.0.8.0 x64 组件".into());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Request {
    path: PathBuf,
    action: Option<(Tuning, u16)>,
    #[serde(default)]
    values: BTreeMap<Tuning, u16>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Reply {
    pub values: BTreeMap<Tuning, u16>,
    pub error: String,
}
pub fn query(action: Option<(Tuning, u16)>) -> Result<Reply> {
    request(action, BTreeMap::new())
}
pub fn apply_profile(values: &BTreeMap<Tuning, u16>) -> Result<Reply> {
    crate::profiles::validate_audio_values(values)?;
    request(None, values.clone())
}
fn request(action: Option<(Tuning, u16)>, values: BTreeMap<Tuning, u16>) -> Result<Reply> {
    crate::adapters::audio::effects_available()?;
    let config: Config = serde_json::from_slice(
        &std::fs::read(config_path())
            .map_err(|_| "请先连接已安装的 HECATE 音效组件".to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let request = serde_json::to_vec(&Request {
        path: config.path,
        action,
        values,
    })
    .map_err(|e| e.to_string())?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--audio-fx-worker")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Err(e) = child.stdin.take().unwrap().write_all(&request) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
        if start.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("音效组件响应超时，已结束隔离进程".into());
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("音效组件异常退出；音量控制仍可使用".into());
    }
    let reply: Reply =
        serde_json::from_slice(&output.stdout).map_err(|_| "音效组件返回内容无效".to_string())?;
    if !reply.error.is_empty() {
        return Err(reply.error);
    }
    Ok(reply)
}
pub fn worker_main() {
    let result = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(16384)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let req: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        run(req)
    })();
    let reply = match result {
        Ok(reply) => reply,
        Err(error) => Reply {
            error,
            ..Reply::default()
        },
    };
    if let Ok(bytes) = serde_json::to_vec(&reply) {
        let _ = std::io::stdout().write_all(&bytes);
    }
}
type Dev = [u32; 4];
type Init = unsafe extern "system" fn() -> u32;
type Count = unsafe extern "system" fn(i32, *mut u32) -> u32;
type GetDev = unsafe extern "system" fn(i32, i32, *mut Dev) -> u32;
type Property =
    unsafe extern "system" fn(*const Dev, *const u16, *mut *mut c_void, *const c_void, u8) -> u32;
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
struct Sdk {
    uninit: Init,
    count: Count,
    get: GetDev,
    property: Property,
    _library: Library,
}
impl Drop for Sdk {
    fn drop(&mut self) {
        unsafe {
            (self.uninit)();
        }
    }
}
unsafe fn address<T: Copy>(module: HMODULE, name: &'static [u8]) -> Result<T> {
    let p = GetProcAddress(module, name.as_ptr()).ok_or("音效组件缺少必需接口")?;
    Ok(std::mem::transmute_copy(&p))
}
impl Sdk {
    fn open(path: &Path) -> Result<Self> {
        unsafe {
            validate_library(path)?;
            let module = LoadLibraryExW(
                crate::hid::wide(&path.to_string_lossy()).as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                return Err(format!(
                    "无法加载音效组件：{}",
                    std::io::Error::last_os_error()
                ));
            }
            let library = Library(module);
            let init: Init = address(module, b"ConfLibInit\0")?;
            let uninit: Init = address(module, b"ConfLibUnInit\0")?;
            let count = address(module, b"GetDeviceCount\0")?;
            let get = address(module, b"GetDeviceById\0")?;
            let property = address(module, b"PropertyControl\0")?;
            check(init(), "初始化音效")?;
            let sdk = Self {
                uninit,
                count,
                get,
                property,
                _library: library,
            };
            let list: Init = address(module, b"CreateDeviceList\0")?;
            check(list(), "枚举音频设备")?;
            Ok(sdk)
        }
    }
    fn text(&self, dev: &Dev, name: &str) -> Result<String> {
        unsafe {
            let mut buffer = [0u16; 4096];
            let mut ptr = buffer.as_mut_ptr().cast();
            let context = [0u8; 1024];
            check(
                (self.property)(
                    dev,
                    crate::hid::wide(name).as_ptr(),
                    &mut ptr,
                    context.as_ptr().cast(),
                    0,
                ),
                name,
            )?;
            let len = buffer
                .iter()
                .position(|&v| v == 0)
                .ok_or("音效组件字符串缺少结束标记")?;
            String::from_utf16(&buffer[..len]).map_err(|e| e.to_string())
        }
    }
    fn get_value(&self, dev: &Dev, setting: &FxSpec) -> Result<u16> {
        let raw = self
            .text(dev, setting.property)?
            .parse::<i32>()
            .map_err(|_| "音效参数不是整数")?;
        encode(setting, raw)
    }
    fn set_value(&self, dev: &Dev, setting: &FxSpec, value: u16) -> Result<()> {
        unsafe {
            if value > setting.max {
                return Err("音效参数超出范围".into());
            }
            let raw = value as i32 - setting.offset;
            let mut data = [0u8; 1024];
            data[..4].copy_from_slice(&raw.to_le_bytes());
            let mut ptr = data.as_mut_ptr().cast();
            let context = [0u8; 1024];
            check(
                (self.property)(
                    dev,
                    crate::hid::wide(setting.property).as_ptr(),
                    &mut ptr,
                    context.as_ptr().cast(),
                    1,
                ),
                setting.property,
            )?;
            if self.get_value(dev, setting)? != value {
                return Err("音效参数写入后读回不一致".into());
            }
            Ok(())
        }
    }
    fn devices(&self) -> Result<BTreeMap<usize, Dev>> {
        unsafe {
            let mut found = BTreeMap::new();
            for flow in [0, 1] {
                let mut count = 0;
                check((self.count)(flow, &mut count), "读取设备数量")?;
                if count > 64 {
                    return Err("音效设备数量异常".into());
                }
                for id in 0..count {
                    let mut dev = [0; 4];
                    check((self.get)(flow, id as i32, &mut dev), "读取音频设备")?;
                    let device_id = self.text(&dev, "GetDeviceID")?.to_lowercase();
                    if !device_id.starts_with("usb\\vid_35bb&pid_a100&mi_00\\") {
                        continue;
                    }
                    let name = self.text(&dev, "GetDeviceFriendlyName")?.to_uppercase();
                    let channel = if flow == 1 {
                        2
                    } else if name.contains("GAMING") {
                        0
                    } else if name.contains("CHAT") {
                        1
                    } else {
                        continue;
                    };
                    if found.insert(channel, dev).is_some() {
                        return Err("检测到多个相同耳机通道，暂不写入".into());
                    }
                }
            }
            if !found.contains_key(&0) {
                return Err("音效组件未找到 G2 Pro GAMING 通道".into());
            }
            Ok(found)
        }
    }
}
fn encode(setting: &FxSpec, raw: i32) -> Result<u16> {
    let v = raw + setting.offset;
    if v < 0 || v > setting.max as i32 {
        return Err(format!("{} 返回了未识别的数值 {raw}", setting.property));
    }
    Ok(v as u16)
}
fn check(code: u32, operation: &str) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{operation} 失败（{code:#x}）"))
    }
}
fn read_values(sdk: &Sdk, devices: &BTreeMap<usize, Dev>) -> BTreeMap<Tuning, u16> {
    SPECS
        .iter()
        .filter_map(|setting| {
            devices
                .get(&setting.channel)
                .and_then(|dev| sdk.get_value(dev, setting).ok())
                .map(|v| (setting.key, v))
        })
        .collect()
}
fn apply_requested<F>(
    desired: &BTreeMap<Tuning, u16>,
    baseline: &BTreeMap<Tuning, u16>,
    mut write: F,
) -> Result<()>
where
    F: FnMut(Tuning, u16) -> Result<()>,
{
    for (key, value) in desired {
        let setting = spec(*key).ok_or("未开放的音效参数")?;
        if *value > setting.max {
            return Err(format!("{key:?} 超出允许范围"));
        }
        if !baseline.contains_key(key) {
            return Err(format!("当前驱动未提供 {key:?}，没有修改任何设置"));
        }
    }
    let mut plan: Vec<_> = desired
        .iter()
        .filter(|(key, value)| baseline.get(key) != Some(*value))
        .map(|(key, value)| (*key, *value))
        .collect();
    // Turn requested effects off before parameters; turn them on only after parameters.
    plan.sort_by_key(|(key, value)| {
        if spec(*key).unwrap().max == 1 {
            if *value == 0 {
                0
            } else {
                2
            }
        } else {
            1
        }
    });
    let mut attempted = Vec::new();
    for (key, value) in plan {
        attempted.push(key);
        if let Err(error) = write(key, value) {
            let mut errors = Vec::new();
            for key in attempted.into_iter().rev() {
                if let Err(e) = write(key, baseline[&key]) {
                    errors.push(e);
                }
            }
            return if errors.is_empty() {
                Err(format!("{error}；已恢复原设置"))
            } else {
                Err(format!("{error}；部分设置恢复失败：{}", errors.join("；")))
            };
        }
    }
    Ok(())
}
fn run(req: Request) -> Result<Reply> {
    let mut desired = req.values;
    if let Some((key, value)) = req.action {
        if !desired.is_empty() {
            return Err("音效请求同时包含单项与配置，已拒绝执行".into());
        }
        desired.insert(key, value);
    }
    if !desired.is_empty() {
        crate::profiles::validate_audio_values(&desired)?;
    }
    let sdk = Sdk::open(&req.path)?;
    let devices = sdk.devices()?;
    let baseline = read_values(&sdk, &devices);
    if baseline.is_empty() {
        return Err("未读取到可用音效，请检查原驱动和音效组件是否安装完整".into());
    }
    apply_requested(&desired, &baseline, |key, value| {
        let setting = spec(key).unwrap();
        let dev = devices.get(&setting.channel).ok_or("音频通道未连接")?;
        sdk.set_value(dev, setting, value)
    })?;
    Ok(Reply {
        values: read_values(&sdk, &devices),
        error: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_failure_restores_every_attempted_parameter() {
        let baseline = BTreeMap::from([
            (Tuning::Eq30, 20),
            (Tuning::BassLevel, 5),
            (Tuning::EqEnabled, 0),
        ]);
        let wanted = BTreeMap::from([
            (Tuning::Eq30, 21),
            (Tuning::BassLevel, 7),
            (Tuning::EqEnabled, 1),
        ]);
        let mut actual = baseline.clone();
        let mut failed = false;
        let result = apply_requested(&wanted, &baseline, |key, value| {
            actual.insert(key, value);
            if key == Tuning::BassLevel && value == 7 && !failed {
                failed = true;
                Err("injected read-back failure".into())
            } else {
                Ok(())
            }
        });
        assert!(result.unwrap_err().contains("已恢复原设置"));
        assert_eq!(actual, baseline);
        let unknown = BTreeMap::from([(Tuning::LightColor, 0)]);
        let mut calls = 0;
        assert!(apply_requested(&unknown, &baseline, |_, _| {
            calls += 1;
            Ok(())
        })
        .is_err());
        assert_eq!(calls, 0);
    }
    #[test]
    fn signed_eq_is_bounded_and_roundtrips() {
        let s = spec(Tuning::Eq30).unwrap();
        for raw in [-20, -1, 0, 1, 20] {
            let v = encode(s, raw).unwrap();
            assert_eq!(v as i32 - s.offset, raw);
        }
        assert!(encode(s, -21).is_err());
        assert!(encode(s, 21).is_err());
    }
    #[test]
    fn settings_have_unique_keys_and_channels() {
        let mut keys = std::collections::BTreeSet::new();
        for s in SPECS {
            assert!(keys.insert(s.key));
            assert!(s.channel < 3);
            assert!(s.max > 0);
        }
    }
}
