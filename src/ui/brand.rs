use crate::devices::DeviceSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Brand {
    Razer,
    Logitech,
}

impl Brand {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Razer => "雷蛇",
            Self::Logitech => "罗技",
        }
    }

    pub(super) fn back_label(self) -> &'static str {
        match self {
            Self::Razer => "‹  雷蛇",
            Self::Logitech => "‹  罗技",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Razer => "选择雷蛇设备，调整该型号支持的指针、按键或灯光设置。",
            Self::Logitech => "选择罗技设备，查看和切换受支持鼠标的板载配置与 DPI 档位。",
        }
    }

    pub(super) fn vendor(self) -> &'static str {
        match self {
            Self::Razer => "razer",
            Self::Logitech => "logitech",
        }
    }

    pub(super) fn for_device(spec: &DeviceSpec) -> Option<Self> {
        let name = spec.brand.trim();
        if name.eq_ignore_ascii_case("Razer") || name == "雷蛇" {
            Some(Self::Razer)
        } else if name.eq_ignore_ascii_case("Logitech") || matches!(name, "罗技" | "羅技") {
            Some(Self::Logitech)
        } else {
            None
        }
    }
}
