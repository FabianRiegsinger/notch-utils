//! Keep Awake: stop the computer from going to sleep for 15 min, 1 h or until turned off.

mod inhibit;

use super::{Ctx, Hud, Module};
use crate::{AwakeState, NotchWindow};
use inhibit::Inhibitor;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

/// "42 min", "1 h 05 min"
pub fn remaining_text(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    if m >= 60 {
        format!("{} h {:02} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

pub struct Awake {
    me: Weak<Awake>,
    ctx: Ctx,
    inhibitor: RefCell<Option<Inhibitor>>,
    /// None = until turned off.
    until: Cell<Option<Instant>>,
    failed: Cell<bool>,
    tick: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Awake {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            inhibitor: RefCell::default(),
            until: Cell::new(None),
            failed: Cell::new(false),
            tick: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn on(&self) -> bool {
        self.inhibitor.borrow().is_some()
    }

    /// minutes: 0 = until turned off, negative = turn off.
    fn set(&self, minutes: i32) {
        if minutes < 0 {
            self.inhibitor.borrow_mut().take();
            self.tick.stop();
        } else {
            if !self.on() {
                *self.inhibitor.borrow_mut() = Inhibitor::new();
            }
            self.failed.set(!self.on());
            self.until.set(
                (minutes > 0).then(|| Instant::now() + Duration::from_secs(minutes as u64 * 60)),
            );
            let me = self.me.clone();
            self.tick.start(
                slint::TimerMode::Repeated,
                Duration::from_secs(20),
                move || {
                    if let Some(a) = me.upgrade() {
                        if a.until.get().is_some_and(|u| Instant::now() >= u) {
                            a.set(-1);
                        } else {
                            a.sync();
                        }
                    }
                },
            );
        }
        self.sync();
    }

    fn status(&self) -> String {
        match (self.on(), self.until.get()) {
            (false, _) if self.failed.get() => "Couldn't keep the computer awake".into(),
            (false, _) => "Off".into(),
            (true, None) => "On until you turn it off".into(),
            (true, Some(u)) => format!(
                "On · {} left",
                remaining_text(u.saturating_duration_since(Instant::now()))
            ),
        }
    }

    fn sync(&self) {
        let (on, status) = (self.on(), self.status());
        let mode = match self.until.get() {
            None => 0,
            Some(u) => (u.saturating_duration_since(Instant::now()).as_secs() > 15 * 60) as i32 + 1,
        };
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<AwakeState>();
            g.set_on(on);
            g.set_status(status.as_str().into());
            g.set_mode(if on { mode } else { -1 });
            true
        });
        (self.ctx.peek_changed)();
    }
}

impl Module for Awake {
    fn id(&self) -> &'static str {
        "awake"
    }

    fn title(&self) -> &'static str {
        "Keep Awake"
    }

    fn attach(&self, ui: &NotchWindow) {
        let me = self.me.clone();
        ui.global::<AwakeState>().on_set(move |minutes| {
            if let Some(a) = me.upgrade() {
                a.set(minutes)
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn config_changed(&self, cfg: &crate::config::Config) {
        // Disabling the module releases the inhibitor.
        if !cfg.layout.modules.iter().any(|m| m == "awake") && self.on() {
            self.set(-1);
        }
    }

    fn activity(&self) -> Option<Hud> {
        if !self.on() {
            return None;
        }
        Some(Hud {
            icon: "awake",
            text: match self.until.get() {
                None => "∞".into(),
                Some(u) => format!(
                    "{}m",
                    u.saturating_duration_since(Instant::now())
                        .as_secs()
                        .div_ceil(60)
                ),
            },
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining() {
        assert_eq!(remaining_text(Duration::from_secs(61)), "2 min");
        assert_eq!(remaining_text(Duration::from_secs(42 * 60)), "42 min");
        assert_eq!(remaining_text(Duration::from_secs(65 * 60)), "1 h 05 min");
    }
}
