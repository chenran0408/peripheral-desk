use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Pointer,
    Bindings,
    Lighting,
    Family(crate::family_editor::Family),
}

impl Tool {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Pointer => "指针与性能",
            Self::Bindings => "按键与板载设置",
            Self::Lighting => "灯效设置",
            Self::Family(family) => family.title(),
        }
    }
    pub(super) unsafe fn open(self, hwnd: HWND, model: usize, bounds: RECT) -> Result<(), String> {
        match self {
            Self::Pointer => pointer_editor::open_embedded(hwnd, &REGISTRY[model], bounds),
            Self::Bindings => crate::controls_ui::open_embedded(hwnd, model, bounds),
            Self::Lighting => crate::generic_lighting_ui::open_embedded(hwnd, model, bounds),
            Self::Family(family) => crate::family_editor::open(hwnd, model, bounds, family),
        }
    }
    pub(super) unsafe fn resize(self, bounds: RECT) {
        match self {
            Self::Pointer => pointer_editor::resize_embedded(bounds),
            Self::Bindings => crate::controls_ui::resize_embedded(bounds),
            Self::Lighting => crate::generic_lighting_ui::resize_embedded(bounds),
            Self::Family(_) => crate::family_editor::resize(bounds),
        }
    }
    pub(super) unsafe fn close(self) -> bool {
        match self {
            Self::Pointer => pointer_editor::close_embedded(),
            Self::Bindings => crate::controls_ui::close_embedded(),
            Self::Lighting => crate::generic_lighting_ui::close_embedded(),
            Self::Family(_) => crate::family_editor::close(),
        }
    }
    unsafe fn busy(self) -> bool {
        match self {
            Self::Pointer => pointer_editor::embedded_writing(),
            Self::Bindings => crate::controls_ui::embedded_writing(),
            Self::Lighting => crate::generic_lighting_ui::embedded_writing(),
            Self::Family(_) => crate::family_editor::writing(),
        }
    }
}

impl App {
    pub(super) fn parent_page(&self) -> Page {
        match self.page {
            Page::Device(_) | Page::Tool(_, Tool::Family(_)) => Page::Drivers,
            Page::Onboard(m) if REGISTRY[m].read_only() => Page::Drivers,
            Page::Onboard(m) | Page::Tool(m, _) => Page::Device(m),
            Page::MHub => self.hub_source,
            _ => Page::Home,
        }
    }
    pub(super) unsafe fn tool_busy(&self) -> bool {
        matches!(self.page,Page::Tool(_,tool) if tool.busy())
    }
    pub(super) unsafe fn open_tool(&mut self, hwnd: HWND, model: usize, tool: Tool) {
        self.poll_skip.insert(model);
        self.navigate(hwnd, Page::Tool(model, tool));
    }
    pub(super) fn grid_entries(&self) -> Vec<Command> {
        match self.page {
            Page::Drivers => self.driver_entries(),
            _ => self.battery_entries(),
        }
    }
    pub(super) fn grid_page_mut(&mut self) -> &mut usize {
        match self.page {
            Page::Drivers => &mut self.drivers_page,
            _ => &mut self.home_page,
        }
    }
}
