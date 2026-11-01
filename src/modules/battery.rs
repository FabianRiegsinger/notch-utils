//! Battery: charge, state, time to full/empty, power source and Low Power Mode.
//! Polls every 10 s, only while visible.

use super::{Ctx, Module};
use crate::{BatteryState, NotchWindow};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(10);

/// "25 min", "2 h 10 min"
fn span(secs: f32) -> String {
    let m = (secs / 60.0).round() as i64;
    if m >= 60 {
        format!("{} h {:02} min", m / 60, m % 60)
    } else {
        format!("{} min", m.max(1))
    }
}

/// `pmset -g ac` → adapter watts ("Wattage = 140W").
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_watts(pmset_ac: &str) -> Option<u32> {
    pmset_ac.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Wattage = ")?
            .trim_end_matches('W')
            .parse()
            .ok()
    })
}

#[cfg(target_os = "macos")]
fn adapter_watts() -> Option<u32> {
    let out = std::process::Command::new("pmset")
        .args(["-g", "ac"])
        .output()
        .ok()?;
    parse_watts(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "macos"))]
fn adapter_watts() -> Option<u32> {
    None
}

/// Low Power Mode / power saver; None where the OS has no such switch we can read.
#[cfg(target_os = "macos")]
fn low_power() -> Option<bool> {
    Some(objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled())
}

#[cfg(target_os = "linux")]
fn low_power() -> Option<bool> {
    let out = std::process::Command::new("powerprofilesctl")
        .arg("get")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim() == "power-saver")
}

#[cfg(target_os = "windows")]
fn low_power() -> Option<bool> {
    None
}

/// macOS and Windows: the OS settings page (changing it needs admin rights or the OS UI).
/// Linux: power-profiles-daemon switches without root.
fn toggle_low_power(on_now: bool) {
    #[cfg(target_os = "macos")]
    let r = {
        let _ = on_now;
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.Battery-Settings.extension")
            .spawn()
    };
    #[cfg(target_os = "linux")]
    let r = std::process::Command::new("powerprofilesctl")
        .args(["set", if on_now { "balanced" } else { "power-saver" }])
        .spawn();
    #[cfg(target_os = "windows")]
    let r = {
        let _ = on_now;
        std::process::Command::new("explorer")
            .arg("ms-settings:batterysaver")
            .spawn()
    };
    if let Err(e) = r {
        log::warn!("battery: low power switch: {e}");
    }
}

pub struct Battery {
    me: Weak<Battery>,
    manager: RefCell<Option<starship_battery::Manager>>,
    timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Battery {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            manager: RefCell::default(),
            timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn sync(&self) {
        use starship_battery::State;
        let b = self
            .manager
            .borrow()
            .as_ref()
            .and_then(|m| m.batteries().ok()?.next()?.ok());
        let low = low_power();
        let watts = adapter_watts();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<BatteryState>();
            g.set_present(b.is_some());
            g.set_has_low_power(low.is_some());
            g.set_low_power(low.unwrap_or(false));
            let Some(b) = &b else { return true };
            let pct = (b.state_of_charge().value * 100.0).round() as i32;
            let state = b.state();
            let plugged = !matches!(state, State::Discharging | State::Empty);
            g.set_percent(pct);
            g.set_charging(state == State::Charging);
            g.set_plugged(plugged);
            g.set_status(
                match state {
                    State::Charging => "Charging",
                    State::Full => "Fully charged",
                    State::Discharging | State::Empty => "On battery",
                    _ => "Not charging",
                }
                .into(),
            );
            let (label, time) = match state {
                State::Charging => ("Until full", b.time_to_full().map(|t| span(t.value))),
                State::Discharging | State::Empty => {
                    ("Time left", b.time_to_empty().map(|t| span(t.value)))
                }
                State::Full => ("Until full", Some("Charged".into())),
                // Plugged in but held (optimized charging, charge limit).
                _ => ("Charging", Some("Paused".into())),
            };
            g.set_time_label(label.into());
            g.set_time(time.unwrap_or_else(|| "–".into()).into());
            let rate = b.energy_rate().value;
            g.set_power(
                match (plugged, watts) {
                    (true, Some(w)) => format!("{w} W adapter"),
                    (true, None) => "Power adapter".into(),
                    (false, _) if rate > 0.1 => format!("Battery · {rate:.1} W"),
                    (false, _) => "Battery".into(),
                }
                .into(),
            );
            true
        });
    }
}

impl Module for Battery {
    fn id(&self) -> &'static str {
        "battery"
    }

    fn title(&self) -> &'static str {
        "Battery"
    }

    fn attach(&self, ui: &NotchWindow) {
        let me = self.me.clone();
        ui.global::<BatteryState>().on_low_power_clicked(move || {
            toggle_low_power(low_power().unwrap_or(false));
            if let Some(b) = me.upgrade() {
                b.sync()
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
    }

    fn on_show(&self) {
        if self.manager.borrow().is_none() {
            *self.manager.borrow_mut() = starship_battery::Manager::new().ok();
        }
        self.sync();
        let me = self.me.clone();
        self.timer.start(slint::TimerMode::Repeated, POLL, move || {
            if let Some(b) = me.upgrade() {
                b.sync()
            }
        });
    }

    fn on_hide(&self) {
        self.timer.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_watts() {
        assert_eq!(span(25.0 * 60.0), "25 min");
        assert_eq!(span(10.0), "1 min");
        assert_eq!(span(130.0 * 60.0), "2 h 10 min");
        assert_eq!(
            parse_watts(" Wattage = 140W\n Current = 4990mA\n"),
            Some(140)
        );
        assert_eq!(parse_watts("No adapter attached.\n"), None);
    }
}
