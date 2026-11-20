use super::io::open_handle;
use crate::hid::{self, wide, Interface};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, os::windows::io::AsRawHandle};
use windows_sys::Win32::Devices::{
    DeviceAndDriverInstallation::*, HumanInterfaceDevice::*, Properties::DEVPKEY_Device_InstanceId,
};

#[derive(Clone)]
pub(super) struct Collection {
    pub interface: Interface,
    pub reports: [BTreeMap<u8, Vec<Value>>; 3],
}

#[derive(Clone)]
pub(super) struct Device {
    pub id: String,
    pub collections: Vec<Collection>,
}

impl Device {
    pub fn descriptor(&self) -> Value {
        let first = &self.collections[0].interface;
        let collections: Vec<_> = self.collections.iter().map(|c| {
            let reports = |t: HIDP_REPORT_TYPE| c.reports[t as usize].iter()
                .map(|(id,items)| json!({"reportId":id,"items":items})).collect::<Vec<_>>();
            json!({"usagePage":c.interface.usage_page,"usage":c.interface.usage,"type":1,"children":[],
                "maxInputReportBytes":c.interface.input_len.saturating_sub(1),
                "maxOutputReportBytes":c.interface.output_len.saturating_sub(1),
                "maxFeatureReportBytes":c.interface.feature_len.saturating_sub(1),
                "inputReports":reports(HidP_Input),"outputReports":reports(HidP_Output),"featureReports":reports(HidP_Feature)})
        }).collect();
        json!({"id":self.id,"vendorId":first.vid,"productId":first.pid,"productName":first.product,"collections":collections})
    }
}

fn parent_id(path: &str) -> Option<String> {
    // Windows exposes one path per top-level collection. Its PnP parent is the
    // HID interface, distinct even for two receivers with identical VID/PID.
    let mut instance = [0u16; MAX_DEVICE_ID_LEN as usize];
    let mut size = std::mem::size_of_val(&instance) as u32;
    let (mut kind, mut node, mut parent) = (0, 0, 0);
    unsafe {
        if CM_Get_Device_Interface_PropertyW(
            wide(path).as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            instance.as_mut_ptr().cast(),
            &mut size,
            0,
        ) != CR_SUCCESS
            || CM_Locate_DevNodeW(&mut node, instance.as_ptr(), 0) != CR_SUCCESS
            || CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS
            || CM_Get_Device_IDW(parent, instance.as_mut_ptr(), instance.len() as u32, 0)
                != CR_SUCCESS
        {
            return None;
        }
    }
    Some(String::from_utf16_lossy(
        &instance[..instance.iter().position(|v| *v == 0)?],
    ))
}

struct Parsed(PHIDP_PREPARSED_DATA);
impl Drop for Parsed {
    fn drop(&mut self) {
        unsafe {
            HidD_FreePreparsedData(self.0);
        }
    }
}

fn collection(interface: Interface) -> Option<Collection> {
    let h = open_handle(&interface.path, 0, 0).ok()?;
    unsafe {
        let mut raw = 0;
        if !HidD_GetPreparsedData(h.as_raw_handle(), &mut raw) {
            return None;
        }
        let parsed = Parsed(raw);
        let mut caps = HIDP_CAPS::default();
        if HidP_GetCaps(parsed.0, &mut caps) != HIDP_STATUS_SUCCESS {
            return None;
        }
        let mut reports: [BTreeMap<u8, Vec<Value>>; 3] = Default::default();
        for (t, mut values, mut buttons) in [
            (
                HidP_Input,
                caps.NumberInputValueCaps,
                caps.NumberInputButtonCaps,
            ),
            (
                HidP_Output,
                caps.NumberOutputValueCaps,
                caps.NumberOutputButtonCaps,
            ),
            (
                HidP_Feature,
                caps.NumberFeatureValueCaps,
                caps.NumberFeatureButtonCaps,
            ),
        ] {
            let mut vc = vec![HIDP_VALUE_CAPS::default(); values as usize];
            if values > 0
                && HidP_GetValueCaps(t, vc.as_mut_ptr(), &mut values, parsed.0)
                    != HIDP_STATUS_SUCCESS
            {
                return None;
            }
            let dst = &mut reports[t as usize];
            for v in vc.into_iter().take(values as usize).filter(|v| !v.IsAlias) {
                let (lo, hi) = if v.IsRange {
                    (v.Anonymous.Range.UsageMin, v.Anonymous.Range.UsageMax)
                } else {
                    (v.Anonymous.NotRange.Usage, v.Anonymous.NotRange.Usage)
                };
                dst.entry(v.ReportID).or_default().push(json!({"reportSize":v.BitSize,"reportCount":v.ReportCount,
                    "usageMinimum":((v.UsagePage as u32)<<16)|lo as u32,"usageMaximum":((v.UsagePage as u32)<<16)|hi as u32}));
            }
            let mut bc = vec![HIDP_BUTTON_CAPS::default(); buttons as usize];
            if buttons > 0
                && HidP_GetButtonCaps(t, bc.as_mut_ptr(), &mut buttons, parsed.0)
                    != HIDP_STATUS_SUCCESS
            {
                return None;
            }
            for b in bc.into_iter().take(buttons as usize).filter(|v| !v.IsAlias) {
                let (lo, hi) = if b.IsRange {
                    (b.Anonymous.Range.UsageMin, b.Anonymous.Range.UsageMax)
                } else {
                    (b.Anonymous.NotRange.Usage, b.Anonymous.NotRange.Usage)
                };
                // HIDP_BUTTON_CAPS does not expose field width. In particular
                // the G75's vendor byte arrays appear here as Windows buttons.
                let mut item = json!({"reportCount":b.ReportCount,
                    "usageMinimum":((b.UsagePage as u32)<<16)|lo as u32,"usageMaximum":((b.UsagePage as u32)<<16)|hi as u32});
                if matches!(
                    (interface.vid, interface.pid),
                    (0x3554, 0xfa09) | (0x41e4, 0x2001)
                ) && interface.usage_page == 0xff02
                    && b.ReportID == 0x13
                    && b.ReportCount == 19
                {
                    item["reportSize"] = json!(8);
                }
                dst.entry(b.ReportID).or_default().push(item);
            }
        }
        Some(Collection { interface, reports })
    }
}

pub(super) fn enumerate() -> Result<BTreeMap<String, Device>, String> {
    let mut devices = BTreeMap::<String, Device>::new();
    for i in hid::enumerate()?
        .into_iter()
        .filter(super::allowlist::supported)
    {
        let Some(parent) = parent_id(&i.path) else {
            continue;
        };
        let Some(collection) = collection(i) else {
            continue;
        };
        let id = format!("{:x}", Sha256::digest(parent.as_bytes()));
        devices
            .entry(id.clone())
            .or_insert_with(|| Device {
                id,
                collections: Vec::new(),
            })
            .collections
            .push(collection);
    }
    Ok(devices)
}
