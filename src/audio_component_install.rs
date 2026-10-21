//! Elevated installation of the official HECATE G2 Pro Windows audio components.
//!
//! The normal UI only starts this executable again with the fixed helper switch.
//! Component paths and commands are derived from the installed, hash-verified plugin
//! bundle; callers cannot supply a path or command line.

use serde::{Deserialize, Serialize};
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const PLUGIN_KEY: &str = "hecate-g2-pro";
const REPORT_LIMIT: usize = 16 * 1024;
const HELPER_WAIT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Serialize, Deserialize)]
struct Step {
    name: String,
    status: String,
    output: String,
}

#[derive(Serialize, Deserialize)]
struct InstallReport {
    status: String,
    restart_required: bool,
    steps: Vec<Step>,
    message: String,
}

fn report_path() -> PathBuf {
    crate::backend::data_path()
        .parent()
        .unwrap_or_else(|| Path::new("data"))
        .join("devices")
        .join(PLUGIN_KEY)
        .join("component-install-result.json")
}

fn save_report(report: &InstallReport) -> Result<(), String> {
    let path = report_path();
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(&path, &bytes)
}

fn decode_output(bytes: &[u8]) -> String {
    let decoded = if bytes.starts_with(&[0xff, 0xfe]) {
        let wide = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&wide)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let wide = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&wide)
    } else {
        decode_oem(bytes)
    };
    decoded.replace('\0', "")
}

fn trim_output(bytes: &[u8]) -> String {
    bounded_text(decode_output(bytes))
}

fn bounded_text(mut text: String) -> String {
    if text.len() > REPORT_LIMIT {
        let mut end = REPORT_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[output truncated]");
    }
    text
}

fn decode_oem(bytes: &[u8]) -> String {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetOEMCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_len: i32,
            output: *mut u16,
            output_len: i32,
        ) -> i32;
    }
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let code_page = GetOEMCP();
        let length = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if length <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; length as usize];
        let written = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            length,
        );
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&wide[..written as usize])
    }
}

fn verify_bundle() -> Result<Vec<(String, PathBuf)>, String> {
    let files = crate::plugin_manager::verified_bundled_components(PLUGIN_KEY)?;
    for name in ["sdk", "driver-inf", "extension-inf", "apo-inf"] {
        component_path(&files, name)?;
    }
    Ok(files)
}

fn component_path(components: &[(String, PathBuf)], name: &str) -> Result<PathBuf, String> {
    components
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, path)| path.clone())
        .ok_or_else(|| format!("插件缺少所需组件：{name}"))
}

fn pnputil_path() -> Result<PathBuf, String> {
    let mut buffer = [0u16; 32768];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("无法确定 Windows 系统目录".into());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("pnputil.exe"))
}

fn snapshot_driver_store() -> Result<(), String> {
    let command = Command::new(pnputil_path()?)
        .args(["/enum-drivers"])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法读取当前驱动状态：{e}"))?;
    let mut output = decode_output(&command.stdout);
    if !command.stderr.is_empty() {
        output.push('\n');
        output.push_str(&decode_output(&command.stderr));
    }
    if !command.status.success() {
        return Err(format!("pnputil 无法读取当前驱动状态：{}", output.trim()));
    }
    let normalized = output.replace("\r\n", "\n");
    let matched = normalized
        .split("\n\n")
        .filter(|block| {
            let lower = block.to_ascii_lowercase();
            ["hecate", "c-media", "cmedia", "cm6646", "35bb", "a100"]
                .iter()
                .any(|needle| lower.contains(needle))
        })
        .collect::<Vec<_>>();
    output = if matched.is_empty() {
        "pnputil 未列出匹配的 HECATE G2 Pro / C-Media 驱动包。\n".into()
    } else {
        matched.join("\n\n")
    };
    let path = crate::backend::data_path()
        .parent()
        .unwrap_or_else(|| Path::new("data"))
        .join("devices")
        .join(PLUGIN_KEY)
        .join("component-install-before.txt");
    crate::storage::atomic_write(&path, bounded_text(output).as_bytes())
}

fn install_inf(name: &str, inf: &Path) -> Step {
    let program = match pnputil_path() {
        Ok(program) => program,
        Err(error) => {
            return Step {
                name: name.into(),
                status: "failed".into(),
                output: error,
            }
        }
    };
    let output = Command::new(program)
        .arg("/add-driver")
        .arg(inf)
        .arg("/install")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output();
    match output {
        Err(error) => Step {
            name: name.into(),
            status: "failed".into(),
            output: format!("无法启动 pnputil：{error}"),
        },
        Ok(result) => {
            let raw_code = result.status.code();
            let mut text = trim_output(&result.stdout);
            if !result.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&trim_output(&result.stderr));
            }
            Step {
                name: name.into(),
                status: if result.status.success() {
                    "succeeded"
                } else if raw_code == Some(3010) {
                    "restart_required"
                } else {
                    "failed"
                }
                .into(),
                output: text,
            }
        }
    }
}

fn worker() -> InstallReport {
    let mut report = InstallReport {
        status: "pending".into(),
        restart_required: false,
        steps: Vec::new(),
        message: String::new(),
    };
    if let Err(error) = save_report(&report) {
        report.status = "failed".into();
        report.message = format!("无法初始化安装报告：{error}");
        return report;
    }
    report.status = "failed".into();
    let components = match verify_bundle() {
        Ok(value) => value,
        Err(error) => {
            report.message = error;
            return report;
        }
    };
    for name in ["driver-inf", "extension-inf", "apo-inf"] {
        if let Err(error) = component_path(&components, name) {
            report.message = error;
            return report;
        }
    }
    if let Err(error) = snapshot_driver_store() {
        report.message = error;
        return report;
    }
    for (name, label) in [
        ("driver-inf", "base-audio-driver"),
        ("extension-inf", "device-extension"),
        ("apo-inf", "audio-processing-component"),
    ] {
        let inf = match component_path(&components, name) {
            Ok(path) => path,
            Err(error) => {
                report.message = error;
                break;
            }
        };
        let step = install_inf(label, &inf);
        let success = step.status == "succeeded" || step.status == "restart_required";
        report.restart_required |= step.status == "restart_required";
        report.steps.push(step);
        if !success {
            report.message = format!("{label} 安装失败；已安装的部分会保留，程序不会自动卸载驱动");
            break;
        }
    }
    if report.message.is_empty() && report.steps.len() == 3 {
        report.restart_required = report
            .steps
            .iter()
            .any(|step| step.status == "restart_required");
        report.status = if report.restart_required {
            "restart_required"
        } else {
            "completed"
        }
        .into();
        report.message = if report.restart_required {
            "G2 Pro 音频组件已安装，Windows 需要重启以完成配置".into()
        } else {
            "G2 Pro 音频组件安装完成".into()
        };
    } else if !report.steps.is_empty() {
        let any_installed = report
            .steps
            .iter()
            .any(|step| step.status == "succeeded" || step.status == "restart_required");
        report.status = if any_installed {
            "partial_failure"
        } else {
            "failure"
        }
        .into();
    }
    report
}

struct InstallerLock(windows_sys::Win32::Foundation::HANDLE);
impl InstallerLock {
    fn acquire() -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        unsafe {
            let handle = CreateMutexW(
                std::ptr::null(),
                0,
                crate::hid::wide("Global\\PeripheralDesk.G2.AudioSetup").as_ptr(),
            );
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            match WaitForSingleObject(handle, 0) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(handle)),
                _ => {
                    CloseHandle(handle);
                    Err("另一个音效驱动安装仍在进行，请稍后再试".into())
                }
            }
        }
    }
}
impl Drop for InstallerLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Entry point for the elevated helper process. Returns the process exit code.
pub fn worker_main() -> i32 {
    let _guard = match InstallerLock::acquire() {
        Ok(guard) => guard,
        Err(_) => return 3,
    };
    let report = worker();
    let code = if report.status == "completed" || report.status == "restart_required" {
        0
    } else {
        1
    };
    if let Err(error) = save_report(&report) {
        eprintln!("Unable to write G2 component installation report: {error}");
        return 2;
    }
    code
}

/// Ask Windows for elevation, run this executable's fixed helper mode, and return
/// the helper's verified result text. The wait is bounded; timed-out work continues.
pub fn launch() -> Result<String, String> {
    verify_bundle().map_err(|e| format!("无法安装 HECATE G2 Pro 音频组件：{e}"))?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let verb = crate::hid::wide("runas");
    let file = crate::hid::wide(&executable.to_string_lossy());
    let parameters = crate::hid::wide("--install-g2-audio");
    let mut info: windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW>() as u32;
    info.fMask = windows_sys::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let launched = unsafe { windows_sys::Win32::UI::Shell::ShellExecuteExW(&mut info) };
    if launched == 0 {
        return Err(format!(
            "无法请求管理员权限：{}",
            std::io::Error::last_os_error()
        ));
    }
    if info.hProcess.is_null() {
        return Err("Windows 未能启动安装进程".into());
    }
    let started = Instant::now();
    let wait = unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObject(
            info.hProcess,
            HELPER_WAIT.as_millis() as u32,
        )
    };
    if wait == windows_sys::Win32::Foundation::WAIT_TIMEOUT {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(info.hProcess) };
        return Err(format!(
            "G2 Pro 音频组件仍在安装（已等待 {} 分钟）；安装程序会继续运行，请稍后查看安装报告",
            started.elapsed().as_secs() / 60
        ));
    }
    if wait != windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
        let error = std::io::Error::last_os_error();
        unsafe { windows_sys::Win32::Foundation::CloseHandle(info.hProcess) };
        return Err(format!("等待安装程序时出错：{error}"));
    }
    let mut exit_code = 1u32;
    unsafe {
        windows_sys::Win32::System::Threading::GetExitCodeProcess(info.hProcess, &mut exit_code)
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(info.hProcess) };
    let path = report_path();
    if exit_code == 3 {
        return Err("另一个音效驱动安装仍在进行；未启动重复安装，请稍后查看结果。".into());
    }
    if exit_code != 0 {
        let report = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<InstallReport>(&raw).ok());
        return Err(report
            .map(|report| {
                let details = report
                    .steps
                    .iter()
                    .map(|step| format!("{}：{} — {}", step.name, step.status, step.output))
                    .collect::<Vec<_>>()
                    .join("\n");
                if details.is_empty() {
                    if report.message.is_empty() {
                        format!("安装程序返回错误代码 {exit_code}")
                    } else {
                        report.message
                    }
                } else {
                    format!("{}\n{}", report.message, details)
                }
            })
            .unwrap_or_else(|| format!("安装程序返回错误代码 {exit_code}")));
    }
    let bytes =
        std::fs::read(&path).map_err(|e| format!("安装程序已结束，但无法读取结果报告：{e}"))?;
    let report: InstallReport =
        serde_json::from_slice(&bytes).map_err(|e| format!("安装报告无效：{e}"))?;
    if report.status == "completed" || report.status == "restart_required" {
        Ok(report.message)
    } else {
        Err(format!(
            "{}\n{}",
            report.message,
            report
                .steps
                .iter()
                .map(|s| format!("{}：{} — {}", s.name, s.status, s.output))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

/// Validate package dependencies without asking for elevation or changing Windows.
pub fn check_installed_bundle() -> Result<usize, String> {
    verify_bundle().map(|components| components.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn another_installer_cannot_acquire_the_live_install_lock() {
        let guard = InstallerLock::acquire().unwrap();
        assert!(std::thread::spawn(|| InstallerLock::acquire().is_err())
            .join()
            .unwrap());
        drop(guard);
        assert!(InstallerLock::acquire().is_ok());
    }
    #[test]
    fn installer_output_keeps_chinese_and_bounds_long_messages() {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend("音效安装成功".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(trim_output(&bytes), "音效安装成功");
        assert!(bounded_text("测".repeat(REPORT_LIMIT)).ends_with("[output truncated]"));
    }
}
