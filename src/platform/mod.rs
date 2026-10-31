//! The few OS-specific bits the notch needs. Everything else is shared code.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod other {
    use slint::winit_030::winit;

    /// Always-on-top and skip-taskbar are set via window attributes in main.rs.
    pub fn setup_window(_window: &winit::window::Window) {}

    /// `None` = primary monitor; `Some(i)` = i-th of `available_monitors()`.
    fn monitor(
        window: &winit::window::Window,
        i: Option<usize>,
    ) -> Option<winit::monitor::MonitorHandle> {
        match i {
            Some(i) => window.available_monitors().nth(i),
            None => window.primary_monitor(),
        }
        .or_else(|| window.current_monitor())
    }

    /// Resize, then move to top-center of the given monitor.
    pub fn place_top_center(window: &winit::window::Window, screen: Option<usize>, w: f32, h: f32) {
        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(w, h));
        let Some(m) = monitor(window, screen) else {
            return;
        };
        let scale = m.scale_factor() as f32;
        let x = m.position().x + ((m.size().width as f32 - w * scale) / 2.0) as i32;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, m.position().y));
    }

    /// No reliable global cursor query wired up yet; trust the OS leave events.
    pub fn cursor_inside(_window: &winit::window::Window) -> Option<bool> {
        None
    }

    /// Only Macs have a hardware notch.
    pub fn notch_size(_screen: Option<usize>) -> Option<(f32, f32)> {
        None
    }

    pub fn screen_count(window: &winit::window::Window) -> usize {
        window.available_monitors().count().max(1)
    }

    /// shortcut: follow-mouse needs a global cursor query (GetCursorPos / XQueryPointer);
    /// until then the notch stays on the primary monitor on Windows and Linux.
    pub fn cursor_screen(_window: &winit::window::Window) -> Option<usize> {
        None
    }

    /// Focus is handled by the window manager.
    pub fn activate_app() {}

    /// shortcut: native drag-out exists on macOS only (OLE DoDragDrop / XDND source are
    /// large); elsewhere the shelf copies files to the clipboard instead.
    pub fn start_file_drag(_window: &winit::window::Window, _paths: &[std::path::PathBuf]) -> bool {
        false
    }
}
#[cfg(not(target_os = "macos"))]
pub use other::*;
