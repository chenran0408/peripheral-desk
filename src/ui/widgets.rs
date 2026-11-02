//! Shared design tokens and reusable painted widgets (owner: theme-widgets agent).

#[derive(Default)]
pub(crate) struct WidgetFx {}

impl WidgetFx {
    /// Advance animations; return true while any animation is still running.
    pub(crate) fn tick(&mut self, _hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        false
    }

    pub(crate) fn on_move(&mut self, _x: i32, _y: i32) {}

    pub(crate) fn on_leave(&mut self) {}
}
