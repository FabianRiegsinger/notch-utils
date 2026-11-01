//! AI subscription usage (Claude Pro/Max/Ultra 5-hour and weekly limits), fed by the Claude Code
//! status line bridge (see claude.rs). Persists only percentages and reset times.

use super::{Ctx, Module};
use crate::claude::{self, Usage, Window};
use crate::{AiState, NotchWindow};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

/// Refresh "resets in…" / "as of…" texts while visible.
const TICK: Duration = Duration::from_secs(30);
/// Peek the 5-hour window from this percentage on.
const PEEK_AT: f64 = 80.0;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// "2 h 10 min", "45 min", "3 d 4 h", "now"
pub fn span(secs: i64) -> String {
    if secs <= 0 {
        return "now".into();
    }
    let (d, h, m) = (
        secs / 86_400,
        secs / 3600 % 24,
        (secs / 60 % 60).max(if secs < 60 { 1 } else { 0 }),
    );
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

/// A window whose reset time has passed is back at 0 %.
pub fn effective(w: Option<Window>, at: i64) -> Option<Window> {
    w.map(|w| {
        if w.resets_at <= at {
            Window {
                used_percentage: 0.0,
                ..w
            }
        } else {
            w
        }
    })
}

fn store_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "notch-utils")
        .map(|d| d.data_dir().join("ai-usage.json"))
}

pub struct AiUsage {
    me: Weak<AiUsage>,
    ctx: Ctx,
    usage: RefCell<Option<Usage>>,
    error: RefCell<String>,
    timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl AiUsage {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let usage = store_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok());
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            usage: RefCell::new(usage),
            error: RefCell::default(),
            timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn set_connected(&self, on: bool) {
        let r = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| {
                claude::edit_settings(|s| {
                    if on {
                        claude::connect(s, &exe)
                    } else {
                        claude::disconnect(s)
                    }
                })
            });
        *self.error.borrow_mut() = match r {
            Ok(()) => String::new(),
            Err(e) => {
                log::warn!("claude settings: {e}");
                e.to_string()
            }
        };
        self.sync();
    }

    fn sync(&self) {
        let at = now();
        let u = self.usage.borrow().clone();
        let five = effective(u.as_ref().and_then(|u| u.five_hour), at);
        let week = effective(u.as_ref().and_then(|u| u.seven_day), at);
        let connected = claude::read_settings().is_some_and(|s| claude::is_connected(&s));
        let as_of = u
            .as_ref()
            .map(|u| match at - u.received_at {
                ..60 => "just now".to_string(),
                d => format!("as of {} ago", span(d)),
            })
            .unwrap_or_default();
        let reset = |w: Option<Window>| {
            w.map(|w| format!("resets in {}", span(w.resets_at - at)))
                .unwrap_or_default()
        };
        let pct = |w: Option<Window>| w.map_or(-1.0, |w| w.used_percentage as f32);
        let error = self.error.borrow().clone();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<AiState>();
            g.set_connected(connected);
            g.set_has_data(five.is_some() || week.is_some());
            g.set_five_pct(pct(five));
            g.set_five_reset(reset(five).into());
            g.set_week_pct(pct(week));
            g.set_week_reset(reset(week).into());
            g.set_as_of(as_of.as_str().into());
            g.set_error(error.as_str().into());
            true
        });
        (self.ctx.peek_changed)();
    }
}

impl Module for AiUsage {
    fn id(&self) -> &'static str {
        "ai"
    }

    fn title(&self) -> &'static str {
        "AI usage"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<AiState>();
        let me = self.me.clone();
        g.on_connect(move || {
            if let Some(m) = me.upgrade() {
                m.set_connected(true)
            }
        });
        let me = self.me.clone();
        g.on_disconnect(move || {
            if let Some(m) = me.upgrade() {
                m.set_connected(false)
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn on_show(&self) {
        self.sync();
        let me = self.me.clone();
        self.timer.start(slint::TimerMode::Repeated, TICK, move || {
            if let Some(m) = me.upgrade() {
                m.sync()
            }
        });
    }

    fn on_hide(&self) {
        self.timer.stop();
    }

    fn receive(&self, topic: &str, payload: &str) {
        if topic != "usage" {
            return;
        }
        let Ok(mut u) = serde_json::from_str::<Usage>(payload) else {
            return;
        };
        u.received_at = now();
        if let Some(p) = store_path() {
            let _ = p.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(p, serde_json::to_vec(&u).unwrap_or_default());
        }
        *self.usage.borrow_mut() = Some(u);
        self.sync();
    }

    fn activity(&self) -> Option<super::Hud> {
        let u = self.usage.borrow();
        let w = effective(u.as_ref()?.five_hour, now())?;
        (w.used_percentage >= PEEK_AT).then(|| super::Hud {
            icon: "ai",
            level: None,
            text: format!("{:.0}%", w.used_percentage),
            warn: w.used_percentage >= 95.0,
            ..Default::default()
        })
    }

    fn peek(&self) -> Option<String> {
        let u = self.usage.borrow();
        let w = effective(u.as_ref()?.five_hour, now())?;
        (w.used_percentage >= PEEK_AT).then(|| {
            format!(
                "Claude {:.0}% · resets in {}",
                w.used_percentage,
                span(w.resets_at - now())
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        assert_eq!(span(0), "now");
        assert_eq!(span(30), "1 min");
        assert_eq!(span(45 * 60), "45 min");
        assert_eq!(span(2 * 3600 + 10 * 60), "2 h 10 min");
        assert_eq!(span(3 * 86_400 + 4 * 3600), "3 d 4 h");
    }

    #[test]
    fn expired_window_is_empty() {
        let w = Some(Window {
            used_percentage: 90.0,
            resets_at: 100,
        });
        assert_eq!(effective(w, 50).unwrap().used_percentage, 90.0);
        assert_eq!(effective(w, 100).unwrap().used_percentage, 0.0);
    }
}
