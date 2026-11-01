//! Status hints beside the notch: Caps Lock, microphone/camera in use, connection, Focus (macOS,
//! only with Full Disk Access). Changes flash for 2 s; mic/camera in use and offline stay as a
//! low-priority live activity. No tab of its own.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
use linux as probe;
#[cfg(target_os = "macos")]
use macos as probe;
#[cfg(target_os = "windows")]
use windows as probe;

use super::{Ctx, Hud, Module};
use crate::NotchWindow;
use crate::config::{self, Config};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Status {
    pub caps: Option<bool>,
    pub mic: Option<bool>,
    pub camera: Option<bool>,
    pub online: Option<bool>,
    pub focus: Option<bool>,
}

/// Online = some non-loopback interface with a routable address.
fn online() -> Option<bool> {
    let ifs = if_addrs::get_if_addrs().ok()?;
    Some(ifs.iter().any(|i| {
        !i.is_loopback()
            && match i.ip() {
                std::net::IpAddr::V4(a) => !a.is_link_local(),
                std::net::IpAddr::V6(a) => (a.segments()[0] & 0xffc0) != 0xfe80,
            }
    }))
}

/// macOS DND store: Focus is on while there are assertion records.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn focus_from_assertions(json: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = v.get("data")?.as_array()?;
    Some(data.iter().any(|d| {
        d.get("storeAssertionRecords")
            .and_then(|r| r.as_array())
            .is_some_and(|r| !r.is_empty())
    }))
}

/// Transient hints for what changed between two readings.
pub fn changes(prev: &Status, now: &Status, cfg: &config::Hints) -> Vec<Hud> {
    let mut out = vec![];
    let flipped = |a: Option<bool>, b: Option<bool>| a.is_some() && b.is_some() && a != b;
    if cfg.caps_lock && flipped(prev.caps, now.caps) {
        let on = now.caps == Some(true);
        out.push(Hud {
            icon: "caps",
            text: if on { "Caps Lock on" } else { "Caps Lock off" }.into(),
            ..Default::default()
        });
    }
    if cfg.connection && flipped(prev.online, now.online) {
        let on = now.online == Some(true);
        out.push(Hud {
            icon: if on { "wifi" } else { "wifi-off" },
            text: if on { "Online" } else { "Offline" }.into(),
            warn: !on,
            ..Default::default()
        });
    }
    if cfg.focus && flipped(prev.focus, now.focus) {
        let on = now.focus == Some(true);
        out.push(Hud {
            icon: "moon",
            text: if on { "Focus on" } else { "Focus off" }.into(),
            ..Default::default()
        });
    }
    out
}

pub struct Watcher(Arc<AtomicBool>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// shortcut: polls cheap system queries (Caps 1 s, mic/camera 2 s, network/Focus 5 s) instead
/// of per-platform change notifications.
fn watch(on: impl Fn(Status) + Send + 'static) -> Option<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new()
        .name("status-hints".into())
        .spawn(move || {
            let mut s = Status::default();
            let mut last: Option<Status> = None;
            let mut tick = 0u32;
            while !flag.load(Ordering::Acquire) {
                s.caps = probe::caps_lock();
                if tick.is_multiple_of(2) {
                    s.mic = probe::mic_in_use();
                    s.camera = probe::camera_in_use();
                }
                if tick.is_multiple_of(5) {
                    s.online = online();
                    s.focus = probe::focus_on();
                }
                if last != Some(s) {
                    last = Some(s);
                    on(s);
                }
                tick = tick.wrapping_add(1);
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok()?;
    Some(Watcher(stop))
}

thread_local! {
    static INSTANCE: RefCell<Weak<Hints>> = RefCell::default();
}

pub struct Hints {
    ctx: Ctx,
    cfg: RefCell<config::Hints>,
    status: Cell<Option<Status>>,
    watcher: RefCell<Option<Watcher>>,
}

impl Hints {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let h = Rc::new(Self {
            ctx,
            cfg: RefCell::default(),
            status: Cell::new(None),
            watcher: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&h));
        h
    }

    fn update(&self, now: Status) {
        // No hints for the very first reading (that's the state at startup, not a change).
        if let Some(prev) = self.status.replace(Some(now)) {
            for h in changes(&prev, &now, &self.cfg.borrow()) {
                (self.ctx.hud)(h);
            }
        }
        (self.ctx.peek_changed)();
    }
}

impl Module for Hints {
    fn id(&self) -> &'static str {
        "hints"
    }

    fn title(&self) -> &'static str {
        "Status hints"
    }

    fn has_view(&self) -> bool {
        false
    }

    fn attach(&self, _ui: &NotchWindow) {}

    fn config_changed(&self, cfg: &Config) {
        *self.cfg.borrow_mut() = cfg.modules.hints.clone();
        let on = cfg.layout.modules.iter().any(|m| m == "hints");
        if on && self.watcher.borrow().is_none() {
            *self.watcher.borrow_mut() = watch(|s| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(h) = INSTANCE.with(|i| i.borrow().upgrade()) {
                        h.update(s)
                    }
                });
            });
        } else if !on {
            self.watcher.borrow_mut().take();
            self.status.set(None);
        }
    }

    /// Persistent: offline, then camera, then microphone in use.
    fn activity(&self) -> Option<Hud> {
        let s = self.status.get()?;
        let cfg = self.cfg.borrow();
        if cfg.connection && s.online == Some(false) {
            return Some(Hud {
                icon: "wifi-off",
                text: "Offline".into(),
                warn: true,
                ..Default::default()
            });
        }
        if cfg.mic_camera && s.camera == Some(true) {
            return Some(Hud {
                icon: "camera",
                text: "In use".into(),
                ..Default::default()
            });
        }
        if cfg.mic_camera && s.mic == Some(true) {
            return Some(Hud {
                icon: "mic",
                text: "In use".into(),
                ..Default::default()
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(caps: bool, online: bool) -> Status {
        Status {
            caps: Some(caps),
            online: Some(online),
            ..Default::default()
        }
    }

    #[test]
    fn hints_only_on_change() {
        let cfg = config::Hints::default();
        assert!(changes(&st(false, true), &st(false, true), &cfg).is_empty());
        let h = changes(&st(false, true), &st(true, false), &cfg);
        assert_eq!(
            h.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            ["Caps Lock on", "Offline"]
        );
        assert!(h[1].warn);
        let off = config::Hints {
            caps_lock: false,
            ..Default::default()
        };
        assert_eq!(
            changes(&st(false, true), &st(true, true), &off).len(),
            0,
            "respects settings"
        );
        assert!(
            changes(&Status::default(), &st(true, true), &cfg).is_empty(),
            "unknown → known is no change"
        );
    }

    #[test]
    fn focus_store() {
        assert_eq!(
            focus_from_assertions(r#"{"data":[{"storeAssertionRecords":[{"x":1}]}]}"#),
            Some(true)
        );
        assert_eq!(
            focus_from_assertions(r#"{"data":[{"storeAssertionRecords":[]}]}"#),
            Some(false)
        );
        assert_eq!(focus_from_assertions(r#"{"data":[{}]}"#), Some(false));
        assert_eq!(focus_from_assertions("nope"), None);
    }
}
