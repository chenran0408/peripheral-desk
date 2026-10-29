use windows_sys::Win32::{
    Foundation::ERROR_FILE_NOT_FOUND,
    System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    },
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "PeripheralDesk";

pub fn is_enabled() -> Result<bool, String> {
    unsafe {
        let key_path = crate::hid::wide(RUN_KEY);
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        );
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        if status != 0 {
            return Err(format!("无法读取开机启动设置（{status}）"));
        }

        let name = crate::hid::wide(VALUE_NAME);
        let mut kind = 0;
        let mut length = 0;
        let status = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut length,
        );
        RegCloseKey(key);
        match status {
            0 => Ok(kind == REG_SZ),
            ERROR_FILE_NOT_FOUND => Ok(false),
            code => Err(format!("无法读取开机启动设置（{code}）")),
        }
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    unsafe {
        let key_path = crate::hid::wide(RUN_KEY);
        let mut key: HKEY = std::ptr::null_mut();
        let status = if enabled {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key_path.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };

        if status == ERROR_FILE_NOT_FOUND && !enabled {
            return Ok(());
        }
        if status != 0 {
            return Err(format!("无法修改开机启动设置（{status}）"));
        }

        let name = crate::hid::wide(VALUE_NAME);
        let status = if enabled {
            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    RegCloseKey(key);
                    return Err(format!("无法确定程序位置：{error}"));
                }
            };
            let command = crate::hid::wide(&format!("\"{}\" --hidden", executable.display()));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * std::mem::size_of::<u16>()) as u32,
            )
        } else {
            let status = RegDeleteValueW(key, name.as_ptr());
            if status == ERROR_FILE_NOT_FOUND {
                0
            } else {
                status
            }
        };
        RegCloseKey(key);
        if status == 0 {
            Ok(())
        } else {
            Err(format!("无法修改开机启动设置（{status}）"))
        }
    }
}
