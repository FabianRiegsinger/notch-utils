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

    /// Resize, then move to top-center of the primary monitor (or the window's current one).
    pub fn place_top_center(window: &winit::window::Window, w: f32, h: f32) {
        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(w, h));
        let Some(m) = window
            .primary_monitor()
            .or_else(|| window.current_monitor())
        else {
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
    pub fn notch_size() -> Option<(f32, f32)> {
        None
    }
}
#[cfg(not(target_os = "macos"))]
pub use other::*;
