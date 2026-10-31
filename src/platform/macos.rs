use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Imp, Sel};
use objc2::{ClassType, MainThreadMarker, ffi, sel};
use objc2_app_kit::{NSScreen, NSStatusWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use slint::winit_030::winit;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Float above the menu bar, on every Space, also next to fullscreen apps.
pub fn setup_window(window: &winit::window::Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    ns_window.setLevel(NSStatusWindowLevel + 1);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    ns_window.setHasShadow(false);
    allow_above_menu_bar(&ns_window);
}

/// AppKit's `constrainFrameRect:toScreen:` pushes windows below the menu bar. Add an
/// override returning the frame unchanged to winit's window class. `class_addMethod` is a
/// no-op if the class already has its own implementation (e.g. on the second call).
fn allow_above_menu_bar(ns_window: &NSWindow) {
    unsafe extern "C-unwind" fn unconstrained(
        _this: &NSWindow,
        _cmd: Sel,
        frame: NSRect,
        _screen: *mut NSScreen,
    ) -> NSRect {
        frame
    }

    let sel = sel!(constrainFrameRect:toScreen:);
    let cls = ns_window.class();
    // SAFETY: the type encoding is copied from NSWindow's own method, and `unconstrained`
    // has exactly that signature (NSRect, id) -> NSRect.
    unsafe {
        let original = ffi::class_getInstanceMethod(NSWindow::class(), sel);
        if original.is_null() {
            return;
        }
        let types = ffi::method_getTypeEncoding(original);
        let imp: Imp =
            std::mem::transmute(unconstrained as unsafe extern "C-unwind" fn(_, _, _, _) -> NSRect);
        ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, sel, imp, types);
    }
}

/// Resize and move to top-center of the menu-bar screen in one setFrame call:
/// winit's move/resize get constrained by AppKit to stay below the menu bar.
pub fn place_top_center(window: &winit::window::Window, screen: Option<usize>, w: f32, h: f32) {
    let (Some(ns_window), Some(mtm)) = (ns_window(window), MainThreadMarker::new()) else {
        return;
    };
    let Some(screen) = nth_screen(mtm, screen) else {
        return;
    };
    let f = screen.frame();
    let (w, h) = (w as f64, h as f64);
    // Cocoa origin is bottom-left.
    let origin = NSPoint::new(
        f.origin.x + (f.size.width - w) / 2.0,
        f.origin.y + f.size.height - h,
    );
    ns_window.setFrame_display(NSRect::new(origin, NSSize::new(w, h)), true);
}

fn ns_window(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit hands out a valid NSView pointer for the lifetime of the window,
    // and we are on the main thread (called from the Slint event loop).
    let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
    view.window()
}

/// Hardware notch size in points on the menu-bar screen, `None` on Macs without one.
pub fn notch_size(screen: Option<usize>) -> Option<(f32, f32)> {
    let mtm = MainThreadMarker::new()?;
    let screen = nth_screen(mtm, screen)?;
    let top = screen.safeAreaInsets().top;
    if top <= 0.0 {
        return None;
    }
    let left = screen.auxiliaryTopLeftArea().size.width;
    let right = screen.auxiliaryTopRightArea().size.width;
    let width = screen.frame().size.width - left - right;
    Some((width as f32, top as f32))
}

/// Whether the mouse is really over the window. AppKit sends spurious leave events
/// while a window is resized under a stationary cursor.
pub fn cursor_inside(window: &winit::window::Window) -> Option<bool> {
    let f = ns_window(window)?.frame();
    let p = objc2_app_kit::NSEvent::mouseLocation();
    Some(
        p.x >= f.origin.x
            && p.x <= f.origin.x + f.size.width
            && p.y >= f.origin.y
            && p.y <= f.origin.y + f.size.height,
    )
}

/// `None` = the menu-bar screen (NSScreen.screens[0]).
fn nth_screen(mtm: MainThreadMarker, i: Option<usize>) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    let i = i.unwrap_or(0);
    (i < screens.count()).then(|| screens.objectAtIndex(i))
}

pub fn screen_count(_window: &winit::window::Window) -> usize {
    MainThreadMarker::new().map_or(1, |mtm| NSScreen::screens(mtm).count())
}

/// Index of the screen under the mouse.
pub fn cursor_screen(_window: &winit::window::Window) -> Option<usize> {
    let mtm = MainThreadMarker::new()?;
    let p = objc2_app_kit::NSEvent::mouseLocation();
    NSScreen::screens(mtm).iter().position(|s| {
        let f = s.frame();
        p.x >= f.origin.x
            && p.x < f.origin.x + f.size.width
            && p.y >= f.origin.y
            && p.y < f.origin.y + f.size.height
    })
}

/// Bring the app forward so a normal window (settings) gets focus: the app runs as an
/// accessory app without a Dock icon, which macOS never activates on its own.
pub fn activate_app() {
    if let Some(mtm) = MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).activate();
    }
}
