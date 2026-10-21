//! Add device metadata and a verified matcher here; the home screen and tray
//! enumerate this registry rather than assuming a fixed number of devices.
use crate::{hid::Interface, model::Tuning};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    Razer,
    MchoseMouse,
    CompxKeyboard,
    HecateAudio,
    External,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Keyboard,
    Headset,
}

pub struct DeviceSpec {
    pub key: &'static str,
    pub name: &'static str,
    pub brand: &'static str,
    pub kind: Kind,
    pub adapter: Adapter,
    pub validation: &'static str,
    pub dpi_range: Option<(u16, u16)>,
    pub dpi_step: u16,
    pub rates: &'static [u16],
    pub wired_rates: &'static [u16],
    pub sleeps: &'static [u16],
    pub image_file: &'static str,
    pub image_bytes: &'static [u8],
    pub advanced: &'static [SettingSpec],
    pub matcher: fn(&Interface) -> bool,
    pub extra_matchers: &'static [DeviceMatch],
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

pub static REGISTRY: std::sync::LazyLock<Vec<DeviceSpec>> = std::sync::LazyLock::new(|| {
    // Runtime protocol roots are stable. Unit tests use this same deterministic
    // fixture instead of inspecting the developer's installed plugins.
    #[cfg(any(feature = "driver-runtime", test))]
    {
        legacy_specs()
    }
    #[cfg(all(not(feature = "driver-runtime"), not(test)))]
    {
        host_registry_from_descriptors(crate::plugin_manager::installed_descriptors(), |key| {
            crate::plugin_manager::installed(key)
                .map(|driver| driver.map(|driver| (driver.version, driver.manifest.device)))
        })
    }
});

fn legacy_specs() -> Vec<DeviceSpec> {
    vec![
        DeviceSpec {
            key: "razer-deathadder-v3-pro",
            name: "炼狱蝰蛇 V3 专业版",
            brand: "RAZER",
            kind: Kind::Mouse,
            adapter: Adapter::Razer,
            validation: "已实机验证 · 功能范围见兼容表",
            extra_matchers: &[],
            dpi_range: Some((100, 30000)),
            dpi_step: 50,
            rates: &[125, 500, 1000],
            wired_rates: &[125, 500, 1000],
            sleeps: &[60, 120, 180, 300, 600, 900],
            advanced: RAZER_ADVANCED,
            image_file: "deathadder.png",
            image_bytes: &[],
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
            validation: "已实机验证 · 功能范围见兼容表",
            extra_matchers: &[],
            dpi_range: Some((50, 26000)),
            dpi_step: 50,
            rates: &[125, 500, 1000, 2000, 4000],
            wired_rates: &[125, 500, 1000, 2000, 4000, 8000],
            sleeps: &[0, 60, 120, 180, 300, 600, 900, 1800, 3600],
            advanced: A5_ADVANCED,
            image_file: "a5.png",
            image_bytes: &[],
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
            validation: "已实机验证 · 功能范围见兼容表",
            extra_matchers: &[],
            dpi_range: None,
            dpi_step: 50,
            rates: &[250, 500, 1000],
            wired_rates: &[250, 500, 1000],
            sleeps: &[0, 60, 120, 300, 600, 900, 1200],
            advanced: G75_ADVANCED,
            image_file: "g75.png",
            image_bytes: &[],
            matcher: |i| {
                ((i.vid == 0x3554 && i.pid == 0xfa09) || (i.vid == 0x41e4 && i.pid == 0x2001))
                    && i.usage_page == 0xff02
                    && i.input_len == 20
                    && i.output_len == 20
            },
        },
        DeviceSpec {
            key: "hecate-g2-pro",
            name: "漫步者 G2 Pro",
            brand: "HECATE",
            kind: Kind::Headset,
            adapter: Adapter::HecateAudio,
            validation: "已实机验证 · 功能范围见兼容表",
            extra_matchers: &[],
            dpi_range: None,
            dpi_step: 50,
            rates: &[],
            wired_rates: &[],
            sleeps: &[],
            image_file: "g2pro.png",
            image_bytes: &[],
            advanced: AUDIO_SETTINGS,
            matcher: |i| {
                i.vid == 0x35bb
                    && i.pid == 0xa100
                    && i.usage_page == 12
                    && i.feature_len == 16
                    && i.product.eq_ignore_ascii_case("HECATE G2 PRO")
            },
        },
    ]
}

/// Only installed package descriptors create desktop cards. Catalog entries,
/// cached state and runtime roots never add a device to this collection.
#[cfg(any(not(feature = "driver-runtime"), test))]
fn host_registry_from_descriptors(
    descriptors: impl IntoIterator<Item = serde_json::Value>,
    mut installed: impl FnMut(&str) -> Result<Option<(String, serde_json::Value)>, String>,
) -> Vec<DeviceSpec> {
    host_registry_from_installed(descriptors.into_iter().filter_map(|descriptor| {
        let key = descriptor.get("key").and_then(serde_json::Value::as_str)?;
        // One broken record, or a record changed during startup, cannot hide
        // other valid installations. Use the successfully reread metadata.
        let (version, metadata) = installed(key).ok().flatten()?;
        (metadata.get("key").and_then(serde_json::Value::as_str) == Some(key))
            .then_some((version, metadata))
    }))
}

#[cfg(any(not(feature = "driver-runtime"), test))]
pub(crate) fn host_registry_from_installed(
    installed: impl IntoIterator<Item = (String, serde_json::Value)>,
) -> Vec<DeviceSpec> {
    let mut devices = Vec::new();
    for (version, value) in installed {
        let Ok(metadata) = serde_json::from_value::<DeviceManifest>(value) else {
            continue;
        };
        if devices
            .iter()
            .any(|spec: &DeviceSpec| spec.key == metadata.key)
        {
            continue;
        }
        let Ok(mut spec) = metadata.into_spec() else {
            continue;
        };
        // These released 0.4.x packages predate complete UI metadata. Restore
        // only their missing capability descriptions, never their old matcher
        // or another device. Future packages must supply their own metadata.
        if matches!(version.as_str(), "0.4.0" | "0.4.1") {
            if let Some(legacy) = legacy_specs()
                .into_iter()
                .find(|known| known.key == spec.key && known.kind == spec.kind)
            {
                if spec.dpi_range.is_none() {
                    spec.dpi_range = legacy.dpi_range;
                }
                if spec.rates.is_empty() {
                    spec.rates = legacy.rates;
                }
                if spec.wired_rates.is_empty() {
                    spec.wired_rates = legacy.wired_rates;
                }
                if spec.sleeps.is_empty() {
                    spec.sleeps = legacy.sleeps;
                }
                if spec.advanced.is_empty() {
                    spec.advanced = legacy.advanced;
                }
            }
        }
        devices.push(spec);
    }
    devices.sort_by_key(|spec| spec.key);
    devices
}

impl DeviceSpec {
    #[cfg(feature = "desktop")]
    pub fn supports(&self, capability: &str) -> bool {
        crate::plugin_manager::installed(self.key)
            .ok()
            .flatten()
            .is_some_and(|driver| driver.manifest.capabilities.iter().any(|c| c == capability))
    }
    pub fn read_only(&self) -> bool {
        self.adapter == Adapter::External
            && self.dpi_range.is_none()
            && self.rates.is_empty()
            && self.wired_rates.is_empty()
            && self.sleeps.is_empty()
            && self.advanced.is_empty()
    }

    pub fn matches(&self, i: &Interface) -> bool {
        if self.adapter == Adapter::External {
            self.extra_matchers.iter().any(|m| m.matches(i))
        } else {
            (self.matcher)(i)
        }
    }

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
        key: Tuning::DpiStages,
        title: "DPI 档位数量",
        hint: "启用 1～6 档，保留其他槽位的 DPI 数值",
        choices: &[(1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (6, "6")],
    },
    SettingSpec {
        key: Tuning::SensorRotation,
        title: "传感器角度",
        hint: "按握持习惯调整移动方向，0° 为默认",
        choices: &[
            (0, "−30°"),
            (10, "−20°"),
            (20, "−10°"),
            (30, "0°"),
            (40, "10°"),
            (50, "20°"),
            (60, "30°"),
        ],
    },
    SettingSpec {
        key: Tuning::EsportsMode,
        title: "电竞模式",
        hint: "初代 A5 的设备内置性能开关",
        choices: SWITCH,
    },
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
        key: Tuning::LightColor,
        title: "单色灯光颜色",
        hint: "仅当前支持的单色灯效可调，自定义色会原样保留",
        choices: &[
            (0, "红"),
            (1, "橙"),
            (2, "黄"),
            (3, "绿"),
            (4, "青"),
            (5, "蓝"),
            (6, "紫"),
            (7, "白"),
        ],
    },
    SettingSpec {
        key: Tuning::MacMode,
        title: "Mac 模式",
        hint: "切换设备内置布局，启用时解除 Windows 键锁定",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::MultiColor,
        title: "灯光配色",
        hint: "切换当前灯效单色 / 多色，保留亮度与速度",
        choices: &[(0, "单色"), (1, "多色")],
    },
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
            Action::Dpi(v) => self.dpi_range.is_some_and(|(min, max)| {
                (min..=max).contains(&v) && v % self.dpi_step.max(1) == 0
            }),
            Action::Rate(v) => self.rates_for(state.wired).contains(&v),
            Action::Sleep(v) => self.sleeps.contains(&v),
            Action::Light(v) => {
                self.kind == Kind::Keyboard && LIGHTS.iter().any(|(id, _)| *id == v)
            }
            Action::Stage(v) => (v as usize) < state.dpi_stages.len(),
            Action::Tune(key, v) => {
                self.advanced.iter().any(|setting| {
                    setting.key == key
                        && (if matches!(
                            key,
                            Tuning::GameVolume | Tuning::ChatVolume | Tuning::MicVolume
                        ) {
                            v <= 100
                        } else if self.kind == Kind::Headset && crate::audio_fx::spec(key).is_some()
                        {
                            crate::audio_fx::spec(key).is_some_and(|s| v <= s.max)
                        } else {
                            setting.choices.iter().any(|(value, _)| *value == v)
                        })
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

const VOLUME_CHOICES: &[(u16, &str)] = &[
    (0, "0%"),
    (25, "25%"),
    (50, "50%"),
    (75, "75%"),
    (100, "100%"),
];
pub const AUDIO_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: Tuning::GameVolume,
        title: "GAMING 音量",
        hint: "游戏与媒体播放通道",
        choices: VOLUME_CHOICES,
    },
    SettingSpec {
        key: Tuning::ChatVolume,
        title: "CHAT 音量",
        hint: "语音通信播放通道",
        choices: VOLUME_CHOICES,
    },
    SettingSpec {
        key: Tuning::MicVolume,
        title: "麦克风音量",
        hint: "仅调节输入音量，不启动录音",
        choices: VOLUME_CHOICES,
    },
    SettingSpec {
        key: Tuning::MicMute,
        title: "麦克风静音",
        hint: "静音状态与 Windows 同步",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::EqEnabled,
        title: "启用均衡器",
        hint: "GAMING 通道 · 关闭后保留频段数值",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::Eq30,
        title: "30 Hz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq60,
        title: "60 Hz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq120,
        title: "120 Hz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq250,
        title: "250 Hz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq500,
        title: "500 Hz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq1k,
        title: "1 kHz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq2k,
        title: "2 kHz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq4k,
        title: "4 kHz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq8k,
        title: "8 kHz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::Eq16k,
        title: "16 kHz",
        hint: "十段均衡器",
        choices: &[],
    },
    SettingSpec {
        key: Tuning::SurroundEnabled,
        title: "虚拟环绕",
        hint: "GAMING 通道 · 复用已安装的音效引擎",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::SurroundRoom,
        title: "空间大小",
        hint: "调整虚拟空间的房间尺寸",
        choices: &[(0, "小"), (1, "中"), (2, "大")],
    },
    SettingSpec {
        key: Tuning::SmartVolumeEnabled,
        title: "响度均衡",
        hint: "减少不同声音之间的音量差异",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::SmartVolumeLevel,
        title: "均衡强度",
        hint: "GAMING 通道 · 当前设置会读回确认",
        choices: &[(0, "0"), (25, "25"), (50, "50"), (75, "75"), (100, "100")],
    },
    SettingSpec {
        key: Tuning::BrillianceEnabled,
        title: "声音提亮",
        hint: "增强高频细节",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::BrillianceLevel,
        title: "提亮强度",
        hint: "作用于声音提亮效果",
        choices: &[
            (0, "0"),
            (1, "1"),
            (2, "2"),
            (3, "3"),
            (4, "4"),
            (5, "5"),
            (6, "6"),
            (7, "7"),
            (8, "8"),
            (9, "9"),
            (10, "10"),
        ],
    },
    SettingSpec {
        key: Tuning::BassEnabled,
        title: "动态低音",
        hint: "增强低频听感",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::BassLevel,
        title: "低音强度",
        hint: "保留当前低频截止频率",
        choices: &[
            (0, "0"),
            (1, "1"),
            (2, "2"),
            (3, "3"),
            (4, "4"),
            (5, "5"),
            (6, "6"),
            (7, "7"),
            (8, "8"),
            (9, "9"),
            (10, "10"),
        ],
    },
    SettingSpec {
        key: Tuning::ClarityEnabled,
        title: "人声清晰",
        hint: "突出语音细节",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::ClarityLevel,
        title: "清晰度强度",
        hint: "保留原有噪声抑制参数",
        choices: &[
            (0, "0"),
            (1, "1"),
            (2, "2"),
            (3, "3"),
            (4, "4"),
            (5, "5"),
            (6, "6"),
            (7, "7"),
            (8, "8"),
            (9, "9"),
            (10, "10"),
        ],
    },
    SettingSpec {
        key: Tuning::MicNoise,
        title: "麦克风降噪",
        hint: "作用于麦克风输入，不启动录音",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::ChatNoise,
        title: "通话降噪",
        hint: "作用于 CHAT 播放通道",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::VoiceEnabled,
        title: "麦克风变声",
        hint: "只改变原有输入效果，不开启监听",
        choices: SWITCH,
    },
    SettingSpec {
        key: Tuning::VoiceMode,
        title: "变声模式",
        hint: "C-Media 原有变声效果",
        choices: &[(0, "模式 1"), (1, "模式 2"), (2, "模式 3"), (3, "模式 4")],
    },
];

/// Downloaded metadata describes supported interfaces, not arbitrary executable UI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceMatch {
    pub vid: u16,
    pub pid: u16,
    #[serde(default)]
    pub usage_page: Option<u16>,
    #[serde(default)]
    pub usage: Option<u16>,
    #[serde(default)]
    pub feature_len: Option<u16>,
    #[serde(default)]
    pub input_len: Option<u16>,
    #[serde(default)]
    pub output_len: Option<u16>,
    #[serde(default)]
    pub product_equals: Option<String>,
    #[serde(default)]
    pub path_contains: Option<String>,
}
impl DeviceMatch {
    pub fn matches(&self, i: &Interface) -> bool {
        self.vid == i.vid
            && self.pid == i.pid
            && self.usage_page.is_none_or(|v| v == i.usage_page)
            && self.usage.is_none_or(|v| v == i.usage)
            && self.feature_len.is_none_or(|v| v == i.feature_len)
            && self.input_len.is_none_or(|v| v == i.input_len)
            && self.output_len.is_none_or(|v| v == i.output_len)
            && self
                .product_equals
                .as_ref()
                .is_none_or(|v| v.eq_ignore_ascii_case(i.product.trim()))
            && self
                .path_contains
                .as_ref()
                .is_none_or(|v| i.path.to_lowercase().contains(&v.to_lowercase()))
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SettingManifest {
    pub key: Tuning,
    pub title: String,
    #[serde(default)]
    pub hint: String,
    pub choices: Vec<(u16, String)>,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DeviceManifest {
    pub key: String,
    pub name: String,
    pub brand: String,
    pub kind: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub image: String,
    pub matches: Vec<DeviceMatch>,
    #[serde(default)]
    pub dpi_range: Option<(u16, u16)>,
    #[serde(default)]
    pub dpi_step: Option<u16>,
    #[serde(default)]
    pub rates: Vec<u16>,
    #[serde(default)]
    pub wired_rates: Vec<u16>,
    #[serde(default)]
    pub sleeps: Vec<u16>,
    #[serde(default)]
    pub advanced: Vec<SettingManifest>,
    #[serde(default)]
    pub validation: serde_json::Value,
}
impl DeviceManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.dpi_step == Some(0)
            || self.dpi_range.is_some_and(|(a, b)| {
                let step = self.dpi_step.unwrap_or(50).max(1);
                a == 0 || b < a || a % step != 0 || b % step != 0
            })
        {
            return Err("设备DPI范围或步进无效".into());
        }
        if self.key.is_empty()
            || self.name.is_empty()
            || self.name.len() > 256
            || self.brand.len() > 128
            || !matches!(self.kind.as_str(), "mouse" | "keyboard" | "headset")
            || self.matches.is_empty()
            || self.matches.len() > 32
            || self.matches.iter().any(|m| m.vid == 0 || m.pid == 0)
            || self.advanced.len() > 64
            || self
                .advanced
                .iter()
                .any(|s| s.choices.is_empty() || s.choices.len() > 128 || s.title.len() > 256)
            || self.rates.len() > 32
            || self.sleeps.len() > 32
        {
            return Err("设备描述字段不支持".into());
        }
        Ok(())
    }
    pub fn into_spec(self) -> Result<DeviceSpec, String> {
        self.validate()?;
        fn text(s: String) -> &'static str {
            Box::leak(s.into_boxed_str())
        }
        let advanced = self
            .advanced
            .into_iter()
            .map(|s| SettingSpec {
                key: s.key,
                title: text(s.title),
                hint: text(s.hint),
                choices: Box::leak(
                    s.choices
                        .into_iter()
                        .map(|(v, s)| (v, text(s)))
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                ),
            })
            .collect::<Vec<_>>();
        Ok(DeviceSpec {
            key: text(self.key),
            name: text(self.name),
            brand: text(self.brand),
            kind: match self.kind.as_str() {
                "keyboard" => Kind::Keyboard,
                "headset" => Kind::Headset,
                _ => Kind::Mouse,
            },
            adapter: Adapter::External,
            validation: match self
                .validation
                .get("status")
                .and_then(serde_json::Value::as_str)
            {
                Some("hardware-verified") => "已实机验证 · 功能范围见兼容表",
                Some("public-implementation-untested") => "依据公开实现适配 · 尚未实机验证",
                _ => "仅识别设备 · 尚未确认功能",
            },
            dpi_range: self.dpi_range,
            dpi_step: self.dpi_step.unwrap_or(50),
            rates: Box::leak(self.rates.into_boxed_slice()),
            wired_rates: Box::leak(self.wired_rates.into_boxed_slice()),
            sleeps: Box::leak(self.sleeps.into_boxed_slice()),
            image_file: "device.png",
            image_bytes: &[],
            advanced: Box::leak(advanced.into_boxed_slice()),
            matcher: |_| false,
            extra_matchers: Box::leak(self.matches.into_boxed_slice()),
        })
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;
    fn a5() -> serde_json::Value {
        serde_json::json!({"key":"mchose-a5-ultra","name":"Installed A5","brand":"MCHOSE","kind":"mouse","matches":[{"vid":14391,"pid":4108,"usage_page":65281,"feature_len":65,"product_equals":"A5 ultra"}]})
    }
    #[test]
    fn a_broken_installed_record_does_not_remove_a_healthy_plugin() {
        let mut broken = a5();
        broken["key"] = serde_json::json!("broken-install");
        let mut looked_up = Vec::new();
        let devices = host_registry_from_descriptors([a5(), broken], |key| {
            looked_up.push(key.to_owned());
            if key == "broken-install" {
                Err("installed.json is malformed".into())
            } else {
                Ok(Some(("0.4.1".into(), a5())))
            }
        });
        assert_eq!(looked_up, ["mchose-a5-ultra", "broken-install"]);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].key, "mchose-a5-ultra");
        assert_eq!(devices[0].dpi_range, Some((50, 26000)));
    }
    #[test]
    fn desktop_registry_has_only_installed_metadata_and_no_four_device_default() {
        assert!(host_registry_from_installed([]).is_empty());
        for version in ["0.4.0", "0.4.1"] {
            let devices = host_registry_from_installed([(version.into(), a5())]);
            assert_eq!(devices.len(), 1);
            let spec = &devices[0];
            assert_eq!(spec.key, "mchose-a5-ultra");
            assert_eq!(spec.name, "Installed A5");
            assert_eq!(spec.adapter, Adapter::External);
            assert_eq!(spec.dpi_range, Some((50, 26000)));
            assert_eq!(spec.advanced.len(), A5_ADVANCED.len());
            assert_eq!(
                spec.extra_matchers.len(),
                1,
                "installed matcher is not replaced by broad legacy matching"
            );
            assert!(!devices.iter().any(|d| matches!(
                d.key,
                "razer-deathadder-v3-pro" | "mchose-g75-pro" | "hecate-g2-pro"
            )));
        }
    }
    #[test]
    fn future_or_different_device_metadata_never_guesses_legacy_capabilities() {
        for version in ["0.4.2", "0.5.0", "0.4.1-preview.1"] {
            let devices = host_registry_from_installed([(version.into(), a5())]);
            assert_eq!(devices.len(), 1);
            assert!(devices[0].read_only());
            assert!(devices[0].advanced.is_empty());
        }
        let mut other_kind = a5();
        other_kind["kind"] = serde_json::json!("headset");
        let devices = host_registry_from_installed([("0.4.1".into(), other_kind)]);
        assert!(devices[0].advanced.is_empty());
        assert_eq!(devices[0].dpi_range, None);
        let mut precise = a5();
        precise["dpi_range"] = serde_json::json!([100, 1000]);
        precise["rates"] = serde_json::json!([125]);
        let devices = host_registry_from_installed([("0.4.1".into(), precise)]);
        assert_eq!(devices[0].dpi_range, Some((100, 1000)));
        assert_eq!(devices[0].rates, &[125]);
    }
    #[test]
    fn a_read_only_plugin_does_not_inherit_writable_mouse_controls() {
        let metadata = serde_json::json!({"key":"test-mouse","name":"Test","brand":"TEST","kind":"mouse","matches":[{"vid":1133,"pid":50495}],"validation":{"status":"public-implementation-untested"}});
        let spec = serde_json::from_value::<DeviceManifest>(metadata.clone())
            .unwrap()
            .into_spec()
            .unwrap();
        assert!(spec.read_only());
        assert!(spec.validation.contains("尚未实机"));
        for action in [
            crate::model::Action::Dpi(800),
            crate::model::Action::Rate(1000),
            crate::model::Action::Sleep(60),
            crate::model::Action::Light(1),
        ] {
            assert!(spec
                .validate(action, &crate::model::State::default())
                .is_err());
        }
        let mut invalid = metadata;
        invalid["dpi_step"] = serde_json::json!(0);
        assert!(serde_json::from_value::<DeviceManifest>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
}
