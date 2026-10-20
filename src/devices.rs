//! Add device metadata and a verified matcher here; the home screen and tray
//! enumerate this registry rather than assuming a fixed number of devices.
use crate::{hid::Interface, model::Tuning};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    Razer,
    MchoseMouse,
    CompxKeyboard,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Keyboard,
}

pub struct DeviceSpec {
    pub key: &'static str,
    pub name: &'static str,
    pub brand: &'static str,
    pub kind: Kind,
    pub adapter: Adapter,
    pub dpi_range: Option<(u16, u16)>,
    pub rates: &'static [u16],
    pub wired_rates: &'static [u16],
    pub sleeps: &'static [u16],
    pub image_file: &'static str,
    pub image_bytes: &'static [u8],
    pub advanced: &'static [SettingSpec],
    pub matcher: fn(&Interface) -> bool,
}

pub const LIGHTS: &[(u8, &str)] = &[
    (0, "关闭"),
    (1, "常亮"),
    (2, "呼吸"),
    (3, "彩虹"),
    (4, "踏雪无痕"),
    (5, "雨中漫步"),
    (7, "涟漪"),
    (8, "繁星"),
    (10, "川流"),
    (11, "潮汐"),
    (12, "如影随形"),
    (13, "正弦波"),
    (15, "风车"),
    (16, "瀑布"),
    (17, "花开"),
];

pub static REGISTRY: &[DeviceSpec] = &[
    DeviceSpec {
        key: "razer-deathadder-v3-pro",
        name: "炼狱蝰蛇 V3 专业版",
        brand: "RAZER",
        kind: Kind::Mouse,
        adapter: Adapter::Razer,
        dpi_range: Some((100, 30000)),
        rates: &[125, 500, 1000],
        wired_rates: &[125, 500, 1000],
        sleeps: &[60, 120, 180, 300, 600, 900],
        advanced: RAZER_ADVANCED,
        image_file: "deathadder.png",
        image_bytes: include_bytes!("../assets/deathadder.png"),
        matcher: |i| {
            i.vid == 0x1532
                && [0x00b6, 0x00b7].contains(&i.pid)
                && i.path.to_lowercase().contains("mi_00")
                && i.feature_len == 91
        },
    },
    DeviceSpec {
        key: "mchose-a5-ultra",
        name: "迈从 A5 Ultra",
        brand: "MCHOSE",
        kind: Kind::Mouse,
        adapter: Adapter::MchoseMouse,
        dpi_range: Some((50, 26000)),
        rates: &[125, 500, 1000, 2000, 4000],
        wired_rates: &[125, 500, 1000, 2000, 4000, 8000],
        sleeps: &[60, 120, 180, 300, 600, 900, 1800, 3600],
        advanced: A5_ADVANCED,
        image_file: "a5.png",
        image_bytes: include_bytes!("../assets/a5.png"),
        matcher: |i| {
            [0x5253, 0x0bda, 0x3837].contains(&i.vid)
                && ["a5 ultra", "mchose a5 ultra"]
                    .contains(&i.product.trim().to_lowercase().as_str())
                && i.usage_page == 0xff01
                && i.feature_len >= 65
        },
    },
    DeviceSpec {
        key: "mchose-g75-pro",
        name: "迈从 G75 Pro",
        brand: "MCHOSE",
        kind: Kind::Keyboard,
        adapter: Adapter::CompxKeyboard,
        dpi_range: None,
        rates: &[250, 500, 1000],
        wired_rates: &[250, 500, 1000],
        sleeps: &[60, 120, 300, 600, 900, 1200],
        advanced: G75_ADVANCED,
        image_file: "g75.png",
        image_bytes: include_bytes!("../assets/g75.png"),
        matcher: |i| {
            ((i.vid == 0x3554 && i.pid == 0xfa09) || (i.vid == 0x41e4 && i.pid == 0x2001))
                && i.usage_page == 0xff02
                && i.input_len == 20
                && i.output_len == 20
        },
    },
];

impl DeviceSpec {
    pub fn rates_for(&self, wired: bool) -> &'static [u16] {
        if wired {
            self.wired_rates
        } else {
            self.rates
        }
    }
}

/// Choice values are protocol-independent; adapters translate and validate them.
pub struct SettingSpec {
    pub key: Tuning,
    pub title: &'static str,
    pub hint: &'static str,
    pub choices: &'static [(u16, &'static str)],
}
const SWITCH: &[(u16, &str)] = &[(0, "关闭"), (1, "开启")];
pub const A5_ADVANCED: &[SettingSpec] = &[
    SettingSpec {
        key: Tuning::Debounce,
        title: "按键防抖",
        hint: "降低数值可缩短响应时间",
        choices: &[
            (0, "0 ms"),
            (2, "2 ms"),
            (4, "4 ms"),
            (8, "8 ms"),
            (12, "12 ms"),
            (16, "16 ms"),
            (20, "20 ms"),
        ],
    },
    SettingSpec {
        key: Tuning::Lod,
        title: "抬升高度 LOD",
        hint: "鼠标离开表面后停止追踪的高度",
        choices: &[(1, "1 mm"), (2, "2 mm")],
    },
    SettingSpec {
        key: Tuning::Motion,
        title: "Motion Sync",
        hint: "使传感器采样与上报节奏同步",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::Angle,
        title: "直线修正",
        hint: "修正细微偏移，使轨迹接近直线",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::Ripple,
        title: "波纹控制",
        hint: "减少高灵敏度时的轨迹抖动",
        choices: SWITCH,
    },
];
pub const G75_ADVANCED: &[SettingSpec] = &[
    SettingSpec {
        key: Tuning::Brightness,
        title: "灯光亮度",
        hint: "作用于当前已开启的灯效",
        choices: &[
            (1, "1"),
            (2, "2"),
            (3, "3"),
            (4, "4"),
            (5, "5"),
            (6, "6"),
            (7, "7"),
            (8, "8"),
            (9, "9"),
        ],
    },
    SettingSpec {
        key: Tuning::Speed,
        title: "灯效速度",
        hint: "作用于当前已开启的灯效",
        choices: &[(1, "1"), (2, "2"), (3, "3"), (4, "4")],
    },
    SettingSpec {
        key: Tuning::WinLock,
        title: "Windows 键锁定",
        hint: "避免游戏中误触 Windows 键",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::Latency,
        title: "低延迟模式",
        hint: "采用设备内置的低延迟设置",
        choices: SWITCH,
    },
];

impl DeviceSpec {
    pub fn validate(
        &self,
        action: crate::model::Action,
        state: &crate::model::State,
    ) -> crate::protocol::Result<()> {
        use crate::model::Action;
        let valid = match action {
            Action::Dpi(v) => self
                .dpi_range
                .is_some_and(|(min, max)| (min..=max).contains(&v) && v % 50 == 0),
            Action::Rate(v) => self.rates_for(state.wired).contains(&v),
            Action::Sleep(v) => self.sleeps.contains(&v),
            Action::Light(v) => {
                self.kind == Kind::Keyboard && LIGHTS.iter().any(|(id, _)| *id == v)
            }
            Action::Stage(v) => (v as usize) < state.dpi_stages.len(),
            Action::Tune(key, v) => {
                self.advanced.iter().any(|setting| {
                    setting.key == key && setting.choices.iter().any(|(value, _)| *value == v)
                }) && state.tuning.contains_key(&key)
            }
        };
        if valid {
            Ok(())
        } else {
            Err("该设备当前不支持这个设置值".into())
        }
    }
}

pub const RAZER_ADVANCED: &[SettingSpec] = &[SettingSpec {
    key: Tuning::LowPower,
    title: "低电量省电阈值",
    hint: "电量低于此值时启用设备自身的省电行为",
    choices: &[
        (5, "5%"),
        (10, "10%"),
        (15, "15%"),
        (20, "20%"),
        (25, "25%"),
        (30, "30%"),
        (40, "40%"),
        (50, "50%"),
    ],
}];
