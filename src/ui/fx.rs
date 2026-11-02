//! Animation and hover plumbing shared by all UI modules. The main window
//! wndproc forwards mouse motion and timer ticks here; each sub-struct owns
//! its own state and returns true while it still needs frames.

#[derive(Default)]
pub(crate) struct Fx {
    pub(crate) widget: super::widgets::WidgetFx,
    pub(crate) nav: super::pages::NavFx,
    pub(crate) flyout: super::tray::TrayFx,
}

impl super::App {
    pub(crate) fn tick_fx(&mut self, hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        let active = self.fx.widget.tick(hwnd) | self.fx.nav.tick(hwnd) | self.fx.flyout.tick(hwnd);
        if !active {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, 3);
            }
        }
        active
    }

    #[allow(dead_code)] // agents call this when starting animations
    pub(crate) fn fx_wake(&self, hwnd: windows_sys::Win32::Foundation::HWND) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, 3, 16, None);
        }
    }

    pub(crate) fn on_hover_move(&mut self, x: i32, y: i32) {
        self.fx.widget.on_move(x, y);
        self.fx.nav.on_move(x, y);
    }

    pub(crate) fn on_hover_leave(&mut self) {
        self.fx.widget.on_leave();
        self.fx.nav.on_leave();
    }
}
