//! Home dashboard: timer, week strip + next event, battery, Claude usage, CPU/RAM. Reads the
//! other modules' Slint globals; owns only the battery reading and the date line.

use super::{Ctx, Module};
use crate::{HomeState, NotchWindow};
use chrono::Local;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

pub struct Home {
    me: Weak<Home>,
    battery: RefCell<Option<starship_battery::Manager>>,
    timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Home {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            battery: RefCell::default(),
            timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn sync(&self) {
        let reading = self
            .battery
            .borrow()
            .as_ref()
            .and_then(|m| m.batteries().ok()?.next()?.ok())
            .map(|b| {
                use starship_battery::State;
                let plugged = !matches!(b.state(), State::Discharging | State::Empty);
                ((b.state_of_charge().value * 100.0).round() as i32, plugged)
            });
        let date = Local::now().format("%A, %-d %B").to_string();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<HomeState>();
            g.set_date(date.as_str().into());
            g.set_has_battery(reading.is_some());
            if let Some((pct, plugged)) = reading {
                g.set_battery_pct(pct);
                g.set_charging(plugged);
            }
            true
        });
    }
}

impl Module for Home {
    fn id(&self) -> &'static str {
        "home"
    }

    fn title(&self) -> &'static str {
        "Home"
    }

    fn embeds(&self) -> &'static [&'static str] {
        &["timer", "calendar", "stats", "ai"]
    }

    fn attach(&self, ui: &NotchWindow) {
        self.windows.borrow_mut().push(ui.as_weak());
    }

    fn on_show(&self) {
        if self.battery.borrow().is_none() {
            *self.battery.borrow_mut() = starship_battery::Manager::new().ok();
        }
        self.sync();
        let me = self.me.clone();
        self.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(30),
            move || {
                if let Some(h) = me.upgrade() {
                    h.sync()
                }
            },
        );
    }

    fn on_hide(&self) {
        self.timer.stop();
    }
}
