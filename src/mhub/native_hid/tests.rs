use super::*;
use crate::hid::Interface;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[test]
#[ignore = "Requires both USB receivers; reads advertised feature reports without issuing a command"]
fn native_bridge_reads_receiver_features_and_releases_handles() {
    let bridge = Bridge::new(|_| {});
    let devices = bridge.request(&json!({"op":"list"})).unwrap();
    for (vid, pid, report) in [(0x3554, 0xfa09, 6), (0x3837, 0x100c, 0x11)] {
        let device = devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["vendorId"] == vid && d["productId"] == pid)
            .unwrap();
        let id = &device["id"];
        let request = json!({"op":"receiveFeature","device":id,"reportId":report});
        assert!(bridge.request(&request).is_err());
        bridge.request(&json!({"op":"open","device":id})).unwrap();
        let bytes = bridge.request(&request).unwrap();
        let bytes: Vec<u8> = serde_json::from_value(bytes).unwrap();
        assert!(bytes.len() > 1);
        println!(
            "{vid:04x}:{pid:04x} feature {report:#04x}: {} bytes",
            bytes.len()
        );
        bridge.request(&json!({"op":"close","device":id})).unwrap();
        assert!(bridge.request(&request).is_err());
    }
    bridge.shutdown();
}

#[test]
#[ignore = "Requires attached G75 Pro and A5 Ultra; reads battery, no settings write"]
fn native_bridge_reads_attached_g75_and_a5() {
    let (tx, rx) = mpsc::channel();
    let bridge = Bridge::new(move |event| {
        let _ = tx.send(event);
    });
    let list = bridge.request(&json!({"op":"list"})).unwrap();
    for d in list.as_array().unwrap() {
        println!(
            "{} {:04x}:{:04x}",
            d["productName"],
            d["vendorId"].as_u64().unwrap(),
            d["productId"].as_u64().unwrap()
        );
    }
    for mouse in [true, false] {
        let device = list
            .as_array()
            .unwrap()
            .iter()
            .find(|d| {
                if mouse {
                    d["vendorId"] == 0x3837 && d["productId"] == 0x100c
                } else {
                    (d["vendorId"] == 0x3554 && d["productId"] == 0xfa09)
                        || (d["vendorId"] == 0x41e4 && d["productId"] == 0x2001)
                }
            })
            .expect("expected test device to be present");
        let id = &device["id"];
        bridge.request(&json!({"op":"open","device":id})).unwrap();
        let report = if mouse {
            crate::protocol::mchose_request(0x11, 6, &[])
        } else {
            let mut p = vec![0; 20];
            p[..3].copy_from_slice(&[0x13, 0x4a, 1]);
            p[19] = 0x13 + 0x4a + 1;
            p
        };
        bridge
            .request(&json!({"op":if mouse {"sendFeature"} else {"sendOutput"},
            "device":id,"reportId":report[0],"data":&report[1..]}))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let matched = loop {
            let evt = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("expected battery response");
            if evt["device"] != *id || evt["op"] != "input" || evt["reportId"] != 0x13 {
                continue;
            }
            let bytes: Vec<u8> = serde_json::from_value(evt["data"].clone()).unwrap();
            if (mouse && bytes.starts_with(&[!0xe5u8, !6u8]))
                || (!mouse && bytes.starts_with(&[0x4a, 1, 0, 2]))
            {
                break bytes;
            }
        };
        if mouse {
            let raw: Vec<u8> = serde_json::from_value(
                bridge
                    .request(&json!({"op":"receiveFeature","device":id,"reportId":0x11}))
                    .unwrap(),
            )
            .unwrap();
            let body = crate::protocol::mchose_reply(&raw[..21], 0x11, 6).unwrap();
            let (battery, _, _) = crate::protocol::mchose_battery(&body).unwrap();
            println!("A5 Ultra confirmed battery {battery}%");
        } else {
            assert!(matched[4] <= 100);
            assert_eq!(matched.len(), 19);
            assert_eq!(
                matched[..18].iter().fold(0x13u8, |s, n| s.wrapping_add(*n)),
                matched[18]
            );
            println!("G75 Pro confirmed battery {}%", matched[4]);
        }
        bridge.request(&json!({"op":"close","device":id})).unwrap();
    }
    bridge.shutdown();
}
#[test]
fn limits_transport_to_pinned_vendor_configuration_collections() {
    let mut i = Interface {
        path: String::new(),
        vid: 0x3837,
        pid: 0x100c,
        usage_page: 0xff01,
        usage: 1,
        feature_len: 65,
        input_len: 65,
        output_len: 65,
        product: "A5 ultra".into(),
    };
    assert!(allowlist::supported(&i));
    i.usage_page = 1;
    assert!(!allowlist::supported(&i));
    i.usage_page = 0xff01;
    i.pid = 0xffff;
    assert!(!allowlist::supported(&i));
    i.vid = 0x41e4;
    i.pid = 0x2102;
    i.usage_page = 0xff00;
    i.product = "Unrelated keyboard".into();
    assert!(!allowlist::supported(&i));
    i.product = "MCHOSE Ace 60".into();
    assert!(allowlist::supported(&i));
    i.usage = 2;
    assert!(!allowlist::supported(&i));
}
