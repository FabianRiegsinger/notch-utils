//! HUD: flashes volume and battery changes in the peek area. No tab of its own.
//! Volume is event-based per OS; battery is a cheap OS query every 5 s.

#[cfg(target_os = "linux")]
mod volume_linux;
#[cfg(target_os = "macos")]
mod volume_macos;
#[cfg(target_os = "windows")]
mod volume_windows;
#[cfg(target_os = "linux")]
use volume_linux as volume;
#[cfg(target_os = "macos")]
use volume_macos as volume;
#[cfg(target_os = "windows")]
use volume_windows as volume;

use super::{Ctx, Hud, Module};
use crate::NotchWindow;
use crate::config::Config;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

const BATTERY_POLL: Duration = Duration::from_secs(5);
const LOW: [f32; 2] = [20.0, 10.0];

thread_local! {
    static INSTANCE: RefCell<Weak<HudModule>> = RefCell::default();
}

pub fn volume_hud(level: f32, muted: bool) -> Hud {
    // Icon ids map to SVGs in app.slint (emoji don't render with the software renderer).
    let icon = if muted || level <= 0.001 {
        "muted"
    } else {
        "volume"
    };
    Hud {
        icon,
        level: Some(if muted { 0.0 } else { level }),
        text: if muted {
            "Muted".into()
        } else {
            format!("{:.0}%", level * 100.0)
        },
        warn: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: f32,
    /// Charger connected (charging, full, or holding charge).
    pub plugged: bool,
    pub full: bool,
}

/// What changed between two battery readings, if worth a HUD.
pub fn battery_hud(prev: Option<Battery>, now: Battery) -> Option<Hud> {
    let prev = prev?;
    let pct = format!("{:.0}%", now.percent);
    if now.plugged != prev.plugged {
        return Some(Hud {
            icon: if now.plugged { "bolt" } else { "battery" },
            level: Some(now.percent / 100.0),
            text: if now.plugged {
                format!("Charging {pct}")
            } else {
                format!("On battery {pct}")
            },
            warn: false,
        });
    }
    if now.full && !prev.full {
        return Some(Hud {
            icon: "battery",
            level: Some(1.0),
            text: "Fully charged".into(),
            warn: false,
        });
    }
    if !now.plugged && LOW.iter().any(|&l| prev.percent > l && now.percent <= l) {
        return Some(Hud {
            icon: "battery-low",
            level: Some(now.percent / 100.0),
            text: format!("Low battery {pct}"),
            warn: true,
        });
    }
    None
}

fn read_battery(m: &starship_battery::Manager) -> Option<Battery> {
    use starship_battery::State;
    let b = m.batteries().ok()?.next()?.ok()?;
    let state = b.state();
    Some(Battery {
        percent: b.state_of_charge().value * 100.0,
        // "Unknown" is what macOS reports while plugged in but holding at a charge limit.
        plugged: !matches!(state, State::Discharging | State::Empty),
        full: state == State::Full,
    })
}

/// "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: ..." → average of the %s.
pub fn parse_pactl_volume(s: &str) -> Option<f32> {
    let v: Vec<f32> = s
        .split('/')
        .filter_map(|p| p.trim().strip_suffix('%')?.trim().parse::<f32>().ok())
        .collect();
    (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32 / 100.0)
}

/// "Mute: no" / "Mute: yes"
pub fn parse_pactl_mute(s: &str) -> Option<bool> {
    match s.trim().strip_prefix("Mute:")?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

pub struct HudModule {
    me: Weak<HudModule>,
    ctx: Ctx,
    enabled: Cell<bool>,
    cfg: RefCell<config::Hud>,
    volume: RefCell<Option<volume::Watcher>>,
    battery: RefCell<Option<starship_battery::Manager>>,
    last_battery: Cell<Option<Battery>>,
    battery_timer: slint::Timer,
}

use crate::config;

impl HudModule {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let m = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            enabled: Cell::new(false),
            cfg: RefCell::default(),
            volume: RefCell::default(),
            battery: RefCell::default(),
            last_battery: Cell::new(None),
            battery_timer: slint::Timer::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&m));
        m
    }

    fn apply(&self) {
        let cfg = self.cfg.borrow().clone();
        let on = self.enabled.get();

        if on && cfg.volume {
            if self.volume.borrow().is_none() {
                *self.volume.borrow_mut() = volume::watch(|level, muted| {
                    // Called on an OS audio thread.
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(m) = INSTANCE.with(|i| i.borrow().upgrade()) {
                            (m.ctx.hud)(volume_hud(level, muted));
                        }
                    });
                });
            }
        } else {
            self.volume.borrow_mut().take();
        }

        if on && cfg.battery {
            if self.battery.borrow().is_none() {
                *self.battery.borrow_mut() = starship_battery::Manager::new().ok();
                self.last_battery
                    .set(self.battery.borrow().as_ref().and_then(read_battery));
                let me = self.me.clone();
                self.battery_timer
                    .start(slint::TimerMode::Repeated, BATTERY_POLL, move || {
                        if let Some(m) = me.upgrade() {
                            m.poll_battery()
                        }
                    });
            }
        } else {
            self.battery_timer.stop();
            self.battery.borrow_mut().take();
            self.last_battery.set(None);
        }
    }

    fn poll_battery(&self) {
        let Some(now) = self.battery.borrow().as_ref().and_then(read_battery) else {
            return;
        };
        if let Some(h) = battery_hud(self.last_battery.replace(Some(now)), now) {
            (self.ctx.hud)(h);
        }
    }
}

impl Module for HudModule {
    fn id(&self) -> &'static str {
        "hud"
    }

    fn title(&self) -> &'static str {
        "Volume & battery HUD"
    }

    fn has_view(&self) -> bool {
        false
    }

    fn attach(&self, _ui: &NotchWindow) {}

    fn config_changed(&self, cfg: &Config) {
        *self.cfg.borrow_mut() = cfg.modules.hud.clone();
        self.enabled
            .set(cfg.layout.modules.iter().any(|m| m == "hud"));
        self.apply();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(percent: f32, plugged: bool, full: bool) -> Battery {
        Battery {
            percent,
            plugged,
            full,
        }
    }

    #[test]
    fn no_hud_on_first_reading_or_no_change() {
        assert_eq!(battery_hud(None, b(50.0, false, false)), None);
        assert_eq!(
            battery_hud(Some(b(51.0, false, false)), b(50.0, false, false)),
            None
        );
    }

    #[test]
    fn charger_events() {
        let h = battery_hud(Some(b(80.0, false, false)), b(80.0, true, false)).unwrap();
        assert_eq!((h.icon, h.text.as_str()), ("bolt", "Charging 80%"));
        let h = battery_hud(Some(b(80.0, true, false)), b(80.0, false, false)).unwrap();
        assert_eq!(h.text, "On battery 80%");
        let h = battery_hud(Some(b(99.0, true, false)), b(100.0, true, true)).unwrap();
        assert_eq!(h.text, "Fully charged");
    }

    #[test]
    fn low_battery_fires_once_per_threshold() {
        let h = battery_hud(Some(b(21.0, false, false)), b(20.0, false, false)).unwrap();
        assert!(h.warn);
        assert_eq!(h.text, "Low battery 20%");
        assert_eq!(
            battery_hud(Some(b(20.0, false, false)), b(19.0, false, false)),
            None
        );
        assert!(battery_hud(Some(b(10.5, false, false)), b(9.8, false, false)).is_some());
        assert_eq!(
            battery_hud(Some(b(21.0, true, false)), b(20.0, true, false)),
            None,
            "not while plugged"
        );
    }

    #[test]
    fn volume_icons() {
        assert_eq!(volume_hud(0.5, false).icon, "volume");
        assert_eq!(volume_hud(0.5, true).text, "Muted");
        assert_eq!(volume_hud(0.9, false).text, "90%");
        assert_eq!(volume_hud(0.0, false).icon, "muted");
    }

    #[test]
    fn pactl_parsing() {
        let v = "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 39321 /  60% / -13.31 dB\n        balance 0.00\n";
        assert!((parse_pactl_volume(v).unwrap() - 0.55).abs() < 1e-6);
        assert_eq!(parse_pactl_volume("garbage"), None);
        assert_eq!(parse_pactl_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_pactl_mute("Mute: no"), Some(false));
        assert_eq!(parse_pactl_mute("Stumm: nein"), None);
    }
}
