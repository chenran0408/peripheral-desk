use super::*;

#[test]
fn native_page_selects_physical_key_and_keeps_the_device_step() {
    unsafe {
        let host = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            980,
            680,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        assert!(!host.is_null());
        let bounds = RECT {
            left: 0,
            top: 0,
            right: 960,
            bottom: 660,
        };
        PAGE.with(|p| {
            p.open(host, 0, bounds, |parent| {
                create(
                    parent,
                    "mhub-ui-fixture",
                    "测试键盘",
                    None,
                    Family::Magnetic,
                    960,
                )
            })
        })
        .unwrap();
        let hwnd = PAGE.with(|p| p.content());
        let editor = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor);
        editor.page=serde_json::from_value(serde_json::json!({
            "section":"rt","sections":[{"id":"rt","label":"触发设置"}],"profile":2,
            "selector":{"label":"选择按键","value":42,"choices":[{"value":42,"label":"A","x":0,"y":0},{"value":17,"label":"B","x":1,"y":0}]},
            "fields":[{"id":"distance","label":"触发行程","value":25,"min":5,"max":400,"step":5,"scale":100,"unit":"mm"}]
        })).unwrap();
        editor.build(hwnd);
        assert_eq!(configuration_ui::text(GetDlgItem(hwnd, KEY as i32)), "A");
        assert_eq!(
            configuration_ui::text(GetDlgItem(hwnd, FIELD as i32)),
            "0.25"
        );
        SetDlgItemTextW(hwnd, FIELD as i32, wide("0.35").as_ptr());
        assert_eq!(editor.changes(hwnd).unwrap()["distance"], 35);
        resize(RECT {
            right: 1100,
            ..bounds
        });
        assert_eq!(
            configuration_ui::text(GetDlgItem(hwnd, FIELD as i32)),
            "0.35"
        );
        let track = GetDlgItem(hwnd, SLIDER as i32);
        SendMessageW(track, TBM_SETPOS, 1, 4);
        SendMessageW(hwnd, WM_HSCROLL, 0, track as isize);
        assert!(editor.changes(hwnd).unwrap().is_empty());
        editor.page=serde_json::from_value(serde_json::json!({
            "section":"eq1","sections":[{"id":"eq1","label":"EQ 1"}],
            "selector":{"label":"频段","value":0,"choices":[{"value":0,"label":"30 Hz"},{"value":1,"label":"60 Hz"}]},
            "curve":[{"value":0,"label":"30","db":-2.0},{"value":1,"label":"60","db":0.0}],
            "fields":[{"id":"gain","label":"增益","value":-20,"min":-120,"max":120,"scale":10,"unit":"dB"}]
        })).unwrap();
        editor.build(hwnd);
        assert_eq!(SendDlgItemMessageW(hwnd, PICK as i32, CB_GETCOUNT, 0, 0), 2);
        assert_eq!(
            configuration_ui::text(GetDlgItem(hwnd, FIELD as i32)),
            "-2.0"
        );
        assert!(close());
        DestroyWindow(host);
    }
}
