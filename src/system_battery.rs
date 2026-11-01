//! Read-only battery sources supplied by Windows rather than device drivers.
use crate::model::State;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use windows::{
    Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice},
    Gaming::Input::{Gamepad, RawGameController},
    System::Power::BatteryStatus,
};
use windows_sys::{
    core::GUID,
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, Properties::*},
        Foundation::DEVPROPKEY,
        UI::Input::XboxController::*,
    },
};

#[derive(Clone, Serialize, Deserialize)]
pub struct SystemBattery {
    pub name: String,
    pub state: State,
    /// XInput exposes a level, not a percentage.
    pub level: Option<String>,
}

pub fn enumerate() -> Vec<SystemBattery> {
    let mut devices = match xbox() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("Windows.Gaming.Input battery enumeration failed: {error}");
            xinput()
        }
    };
    match bluetooth() {
        Ok(mut bluetooth) => devices.append(&mut bluetooth),
        Err(error) => eprintln!("Bluetooth battery enumeration failed: {error}"),
    }
    devices
}

fn state(key: String, percent: Option<u8>, charging: Option<bool>) -> State {
    State {
        key,
        present: true,
        fresh: percent.is_some(),
        percent,
        charging,
        updated: crate::backend::now(),
        ..State::default()
    }
}

fn xbox() -> Result<Vec<SystemBattery>, String> {
    let gamepads = Gamepad::Gamepads().map_err(|e| e.to_string())?;
    let mut devices = Vec::new();
    let mut has_exact_report = false;
    for index in 0..gamepads.Size().map_err(|e| e.to_string())? {
        let gamepad = gamepads.GetAt(index).map_err(|e| e.to_string())?;
        let raw = RawGameController::FromGameController(&gamepad).ok();
        let name = raw
            .as_ref()
            .and_then(|raw| raw.DisplayName().ok())
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Xbox 兼容手柄".into());
        let report = raw
            .as_ref()
            .and_then(|raw| raw.TryGetBatteryReport().ok())
            .or_else(|| gamepad.TryGetBatteryReport().ok());
        let Some(report) = report else {
            continue;
        };
        let remaining = report
            .RemainingCapacityInMilliwattHours()
            .ok()
            .and_then(|value| value.Value().ok());
        let full = report
            .FullChargeCapacityInMilliwattHours()
            .ok()
            .and_then(|value| value.Value().ok());
        let percent = match (remaining, full) {
            (Some(remaining), Some(full)) if full > 0 && remaining >= 0 => {
                Some(((remaining as i64 * 100 / full as i64).clamp(0, 100)) as u8)
            }
            _ => None,
        };
        if percent.is_some() {
            has_exact_report = true;
        }
        let charging = match report.Status().ok() {
            Some(BatteryStatus::Charging) => Some(true),
            Some(BatteryStatus::Discharging) => Some(false),
            _ => None,
        };
        devices.push(SystemBattery {
            name,
            state: state(format!("system-xbox-{index}"), percent, charging),
            level: None,
        });
    }
    // WGI and XInput have no reliable common slot ID. Only switch to XInput
    // when WGI has no exact reading, so the two paths cannot duplicate a pad.
    if !has_exact_report {
        let fallback = xinput();
        if !fallback.is_empty() {
            return Ok(fallback);
        }
    }
    Ok(devices)
}

fn xinput() -> Vec<SystemBattery> {
    let mut devices = Vec::new();
    for index in 0..4 {
        let mut input = XINPUT_STATE::default();
        if unsafe { XInputGetState(index, &mut input) } != 0 {
            continue;
        }
        let mut battery = XINPUT_BATTERY_INFORMATION::default();
        let level =
            if unsafe { XInputGetBatteryInformation(index, BATTERY_DEVTYPE_GAMEPAD, &mut battery) }
                == 0
                && !matches!(
                    battery.BatteryType,
                    BATTERY_TYPE_UNKNOWN | BATTERY_TYPE_DISCONNECTED | BATTERY_TYPE_WIRED
                )
            {
                match battery.BatteryLevel {
                    BATTERY_LEVEL_EMPTY => Some("耗尽"),
                    BATTERY_LEVEL_LOW => Some("低"),
                    BATTERY_LEVEL_MEDIUM => Some("中"),
                    BATTERY_LEVEL_FULL => Some("满"),
                    _ => None,
                }
            } else {
                None
            };
        devices.push(SystemBattery {
            name: format!("Xbox 兼容手柄 {}", index + 1),
            state: state(format!("system-xbox-{index}"), None, None),
            level: level.map(str::to_owned),
        });
    }
    devices
}

// Windows Settings' Bluetooth battery devnode property. It is deliberately
// distinct from the documented Bluetooth address property.
const BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

fn bluetooth() -> Result<Vec<SystemBattery>, String> {
    let mut levels = BTreeMap::<u64, u8>::new();
    for enumerator in ["BthHFEnum", "BTHENUM", "BTHLE"] {
        let filter = crate::hid::wide(enumerator);
        let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
        let mut length = 0;
        let result = unsafe { CM_Get_Device_ID_List_SizeW(&mut length, filter.as_ptr(), flags) };
        if result != CR_SUCCESS {
            return Err(format!(
                "{enumerator}: CM_Get_Device_ID_List_SizeW: {result}"
            ));
        }
        if length <= 1 {
            continue;
        }
        let mut ids = vec![0u16; length as usize];
        let result =
            unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), ids.as_mut_ptr(), length, flags) };
        if result != CR_SUCCESS {
            return Err(format!("{enumerator}: CM_Get_Device_ID_ListW: {result}"));
        }
        for id in ids.split(|&unit| unit == 0).filter(|id| !id.is_empty()) {
            let device_id = String::from_utf16_lossy(id);
            let Some(mac) = mac_in_id(&device_id) else {
                continue;
            };
            let mut devnode = 0;
            let id = crate::hid::wide(&device_id);
            if unsafe { CM_Locate_DevNodeW(&mut devnode, id.as_ptr(), 0) } != CR_SUCCESS {
                continue;
            }
            let mut kind = 0;
            let mut size = 1;
            let mut value = 0u8;
            if unsafe {
                CM_Get_DevNode_PropertyW(
                    devnode,
                    &BLUETOOTH_BATTERY,
                    &mut kind,
                    &mut value,
                    &mut size,
                    0,
                )
            } == CR_SUCCESS
                && kind & DEVPROP_MASK_TYPE == DEVPROP_TYPE_BYTE
                && size == 1
                && value <= 100
            {
                // HFP gets precedence when several devnodes share one address.
                levels.entry(mac).or_insert(value);
            }
        }
    }
    let mut devices = Vec::new();
    for (mac, percent) in levels {
        let classic = BluetoothDevice::FromBluetoothAddressAsync(mac)
            .ok()
            .and_then(|op| op.get().ok());
        let connected = classic
            .as_ref()
            .and_then(|device| device.ConnectionStatus().ok())
            == Some(BluetoothConnectionStatus::Connected);
        let le = if connected {
            None
        } else {
            BluetoothLEDevice::FromBluetoothAddressAsync(mac)
                .ok()
                .and_then(|op| op.get().ok())
        };
        let le_connected = le
            .as_ref()
            .and_then(|device| device.ConnectionStatus().ok())
            == Some(BluetoothConnectionStatus::Connected);
        if !connected && !le_connected {
            continue;
        }
        let name = if connected {
            classic.and_then(|device| device.Name().ok())
        } else {
            le.and_then(|device| device.Name().ok())
        }
        .map(|name| name.to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{mac:012X}"));
        devices.push(SystemBattery {
            name: format!("蓝牙设备 {name}"),
            state: state(format!("system-bt-{mac:012x}"), Some(percent), None),
            level: None,
        });
    }
    Ok(devices)
}

fn mac_in_id(id: &str) -> Option<u64> {
    let upper = id.to_ascii_uppercase();
    let start = upper.find("DEV_")? + 4;
    let address = upper.get(start..start + 12)?;
    address
        .chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| u64::from_str_radix(address, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::mac_in_id;

    #[test]
    fn bluetooth_device_id_requires_a_real_mac() {
        assert_eq!(
            mac_in_id("BTHENUM\\DEV_102030A0B0C0\\7&1"),
            Some(0x102030a0b0c0)
        );
        assert_eq!(mac_in_id("BTHENUM\\SOME_OTHER_DEVICE"), None);
        assert_eq!(mac_in_id("BTHENUM\\DEV_102030A0B0C\\7&1"), None);
    }
}
