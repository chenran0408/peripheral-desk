use super::*;
use std::ffi::c_void;
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::*,
};
pub(super) fn request(
    action: Option<(Tuning, u16)>,
    values: BTreeMap<Tuning, u16>,
) -> Result<Reply> {
    crate::adapters::audio::effects_available()?;
    let config: Config = serde_json::from_slice(
        &std::fs::read(config_path())
            .map_err(|_| "请先连接已安装的 HECATE 音效组件".to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // The host already runs this DLL in its isolated plugin worker and owns the
    // timeout. Keep the SDK in that process so terminating it also releases SDK
    // handles; another worker layer could outlive its timeout monitor.
    run(Request {
        path: config.path,
        action,
        values,
    })
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
pub(super) fn run(req: Request) -> Result<Reply> {
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
