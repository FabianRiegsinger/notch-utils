//! Global hotkey. Re-registered on config change. Not available on Wayland: users bind
//! `notch-utils --toggle` in their desktop instead.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::cell::RefCell;

pub struct Hotkey {
    manager: Option<GlobalHotKeyManager>,
    current: RefCell<Option<HotKey>>,
}

impl Hotkey {
    /// `on_press` runs on whatever thread the OS backend delivers events on.
    pub fn new(on_press: impl Fn() + Send + Sync + 'static) -> Self {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| log::warn!("global hotkeys unavailable: {e}"))
            .ok();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            log::debug!("hotkey event {e:?}");
            if e.state == HotKeyState::Pressed {
                on_press()
            }
        }));
        Self {
            manager,
            current: RefCell::default(),
        }
    }

    /// Register `spec` (e.g. "Alt+Space"), or nothing when disabled. Returns a
    /// user-facing error message on failure.
    pub fn set(&self, enabled: bool, spec: &str) -> Result<(), String> {
        let Some(m) = &self.manager else {
            return if enabled {
                Err("Global hotkeys are not supported here".into())
            } else {
                Ok(())
            };
        };
        if let Some(old) = self.current.borrow_mut().take() {
            let _ = m.unregister(old);
        }
        if !enabled {
            return Ok(());
        }
        let hk: HotKey = spec
            .parse()
            .map_err(|e| format!("Invalid hotkey {spec:?}: {e}"))?;
        m.register(hk)
            .map_err(|e| format!("Could not register {spec:?}: {e}"))?;
        log::debug!("hotkey registered: {spec} ({hk:?})");
        *self.current.borrow_mut() = Some(hk);
        Ok(())
    }
}
