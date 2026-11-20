use crate::protocol::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::*},
    Foundation::*,
    Storage::FileSystem::*,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct Interface {
    #[serde(skip)]
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub feature_len: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub product: String,
}

/// An anonymous Windows device container, shared by its HID collections.
/// No PnP path, serial number or user-defined device name leaves this layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTarget {
    pub id: String,
    pub label: String,
}

impl Interface {
    pub fn instance_id(&self) -> String {
        use windows_sys::Win32::Devices::Properties::{
            DEVPKEY_Device_ContainerId, DEVPKEY_Device_InstanceId, DEVPROP_TYPE_GUID,
        };
        let mut instance = [0u16; MAX_DEVICE_ID_LEN as usize];
        let mut size = std::mem::size_of_val(&instance) as u32;
        let (mut kind, mut node) = (0, 0);
        let mut hash = Sha256::new();
        unsafe {
            if CM_Get_Device_Interface_PropertyW(
                wide(&self.path).as_ptr(),
                &DEVPKEY_Device_InstanceId,
                &mut kind,
                instance.as_mut_ptr().cast(),
                &mut size,
                0,
            ) == CR_SUCCESS
                && CM_Locate_DevNodeW(&mut node, instance.as_ptr(), 0) == CR_SUCCESS
            {
                let mut container = [0u8; 16];
                size = container.len() as u32;
                if CM_Get_DevNode_PropertyW(
                    node,
                    &DEVPKEY_Device_ContainerId,
                    &mut kind,
                    container.as_mut_ptr(),
                    &mut size,
                    0,
                ) == CR_SUCCESS
                    && kind == DEVPROP_TYPE_GUID
                    && size == 16
                    && container != [0; 16]
                    // Windows assigns 00000000-0000-0000-ffff-ffffffffffff to the whole PC.
                    && !(container[..8] == [0; 8] && container[8..] == [0xff; 8])
                {
                    hash.update(container);
                    return format!("{:x}", hash.finalize());
                }
                let mut parent = 0;
                if CM_Get_Parent(&mut parent, node, 0) == CR_SUCCESS
                    && CM_Get_Device_IDW(parent, instance.as_mut_ptr(), instance.len() as u32, 0)
                        == CR_SUCCESS
                {
                    let len = instance
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(instance.len());
                    hash.update(
                        String::from_utf16_lossy(&instance[..len])
                            .to_lowercase()
                            .as_bytes(),
                    );
                    return format!("{:x}", hash.finalize());
                }
            }
        }
        // Some virtual HID collections have no device container.
        hash.update(self.path.to_lowercase().as_bytes());
        format!("{:x}", hash.finalize())
    }

    pub fn target(&self) -> DeviceTarget {
        let id = self.instance_id();
        let label = format!("{:04X}:{:04X} · {}", self.vid, self.pid, &id[..8]);
        DeviceTarget { id, label }
    }
}

/// Optional ABI 1 request extension. Only a missing field selects the legacy route.
pub fn request_instance(request: &serde_json::Value) -> Result<Option<&str>> {
    request
        .get("target_instance")
        .map(|value| {
            value
                .as_str()
                .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| "目标设备标识无效".to_string())
        })
        .transpose()
}

pub struct Device(HANDLE);
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

impl Device {
    pub fn open(path: &str) -> Result<Self> {
        // Zero desired access is intentional: Windows owns mouse/keyboard TLCs,
        // but HID feature IOCTLs remain available on their control handle.
        let handle = unsafe {
            CreateFileW(
                wide(path).as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(format!(
                "打开 HID 接口失败：{}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(Self(handle))
        }
    }

    pub fn send(&self, wire: &[u8]) -> Result<()> {
        if !unsafe { HidD_SetFeature(self.0, wire.as_ptr() as _, wire.len() as u32) } {
            return Err(format!(
                "发送 HID Feature 失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn receive(&self, report: u8, len: usize) -> Result<Vec<u8>> {
        let mut data = vec![0; len];
        data[0] = report;
        if !unsafe { HidD_GetFeature(self.0, data.as_mut_ptr() as _, len as u32) } {
            return Err(format!(
                "读取 HID Feature 失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(data)
    }
}

pub fn enumerate() -> Result<Vec<Interface>> {
    unsafe {
        let mut guid = zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == INVALID_HANDLE_VALUE as isize {
            return Err(format!(
                "枚举 HID 失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        let result = enumerate_set(set, &guid);
        SetupDiDestroyDeviceInfoList(set);
        result
    }
}

unsafe fn enumerate_set(set: HDEVINFO, guid: &windows_sys::core::GUID) -> Result<Vec<Interface>> {
    let mut items = Vec::new();
    for index in 0.. {
        let mut interface: SP_DEVICE_INTERFACE_DATA = zeroed();
        interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if SetupDiEnumDeviceInterfaces(set, null(), guid, index, &mut interface) == 0 {
            if GetLastError() == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(format!(
                "枚举 HID 集合失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        let mut needed = 0;
        SetupDiGetDeviceInterfaceDetailW(set, &interface, null_mut(), 0, &mut needed, null_mut());
        if needed < 8 {
            continue;
        }
        // usize storage guarantees native alignment of the variable-size structure.
        let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        let detail = storage.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        if SetupDiGetDeviceInterfaceDetailW(set, &interface, detail, needed, null_mut(), null_mut())
            == 0
        {
            continue;
        }
        let ptr = (*detail).DevicePath.as_ptr();
        let max_len = (needed as usize - 4) / 2;
        let chars = std::slice::from_raw_parts(ptr, max_len);
        let path = String::from_utf16_lossy(
            &chars[..chars.iter().position(|c| *c == 0).unwrap_or(chars.len())],
        );
        // Enumerate all HID identities; downloaded plugin matchers decide which
        // devices belong to them. Opening with zero access sends no device command.
        let dev = match Device::open(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let mut attrs: HIDD_ATTRIBUTES = zeroed();
        attrs.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        if !HidD_GetAttributes(dev.0, &mut attrs) {
            continue;
        }
        let mut product = [0u16; 256];
        HidD_GetProductString(dev.0, product.as_mut_ptr() as _, 512);
        let product = String::from_utf16_lossy(
            &product[..product.iter().position(|v| *v == 0).unwrap_or(256)],
        );
        let mut caps: HIDP_CAPS = zeroed();
        let mut preparsed = 0;
        if HidD_GetPreparsedData(dev.0, &mut preparsed) {
            HidP_GetCaps(preparsed, &mut caps);
            HidD_FreePreparsedData(preparsed);
        }
        items.push(Interface {
            path,
            vid: attrs.VendorID,
            pid: attrs.ProductID,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            feature_len: caps.FeatureReportByteLength,
            input_len: caps.InputReportByteLength,
            output_len: caps.OutputReportByteLength,
            product,
        });
    }
    Ok(items)
}
