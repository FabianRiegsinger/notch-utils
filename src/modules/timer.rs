//! Countdown timer + stopwatch. Time is computed from `Instant`s, so it stays exact even
//! when no UI refresh happens; the 200 ms tick runs only while the clock is running.

use super::{Ctx, Module};
use crate::{NotchWindow, TimerState};
use slint::{ComponentHandle, SharedString};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

const DEFAULT: Duration = Duration::from_secs(5 * 60);
const MAX: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, PartialEq)]
struct Clock {
    stopwatch: bool,
    /// Countdown length.
    duration: Duration,
    /// Time accumulated before the current run.
    elapsed_before: Duration,
    started: Option<Instant>,
    finished: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            stopwatch: false,
            duration: DEFAULT,
            elapsed_before: Duration::ZERO,
            started: None,
            finished: false,
        }
    }
}

impl Clock {
    fn elapsed(&self, now: Instant) -> Duration {
        self.elapsed_before + self.started.map_or(Duration::ZERO, |s| now - s)
    }

    /// What the big display shows: remaining (countdown) or elapsed (stopwatch).
    fn shown(&self, now: Instant) -> Duration {
        if self.stopwatch {
            self.elapsed(now)
        } else {
            self.duration.saturating_sub(self.elapsed(now))
        }
    }

    fn running(&self) -> bool {
        self.started.is_some()
    }

    fn start_pause(&mut self, now: Instant) {
        self.finished = false;
        match self.started.take() {
            Some(s) => self.elapsed_before += now - s,
            None if !self.stopwatch && self.shown(now).is_zero() => {}
            None => self.started = Some(now),
        }
    }

    fn reset(&mut self) {
        self.elapsed_before = Duration::ZERO;
        self.started = None;
        self.finished = false;
    }

    fn add_minutes(&mut self, n: i32) {
        let delta = Duration::from_secs(60 * n.unsigned_abs() as u64);
        self.duration = if n < 0 {
            self.duration
                .saturating_sub(delta)
                .max(Duration::from_secs(60))
        } else {
            (self.duration + delta).min(MAX)
        };
    }

    /// Returns true exactly once, when a countdown hits zero.
    fn check_finished(&mut self, now: Instant) -> bool {
        if !self.stopwatch && self.running() && self.shown(now).is_zero() {
            self.reset();
            self.finished = true;
            return true;
        }
        false
    }
}

pub fn format(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub struct Timer {
    me: Weak<Timer>,
    ctx: Ctx,
    clock: RefCell<Clock>,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
    tick: slint::Timer,
}

impl Timer {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            clock: RefCell::default(),
            windows: RefCell::default(),
            tick: slint::Timer::default(),
        })
    }

    fn update(&self, f: impl FnOnce(&mut Clock, Instant)) {
        f(&mut self.clock.borrow_mut(), Instant::now());
        self.sync();
    }

    /// Push clock state to every window and (re)arm the tick.
    fn sync(&self) {
        let now = Instant::now();
        let finished = self.clock.borrow_mut().check_finished(now);
        let c = self.clock.borrow().clone();
        let display = SharedString::from(format(c.shown(now)));
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<TimerState>();
            g.set_display(display.clone());
            g.set_progress(if c.stopwatch {
                (c.shown(now).as_secs_f32() % 60.0) / 60.0
            } else {
                c.shown(now).as_secs_f32() / c.duration.as_secs_f32().max(1.0)
            });
            g.set_running(c.running());
            g.set_stopwatch(c.stopwatch);
            true
        });
        if c.running() && !self.tick.running() {
            let me = self.me.clone();
            self.tick.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(200),
                move || {
                    if let Some(t) = me.upgrade() {
                        t.sync()
                    }
                },
            );
        } else if !c.running() {
            self.tick.stop();
        }
        (self.ctx.peek_changed)();
        if finished {
            (self.ctx.alert)();
        }
    }
}

impl Module for Timer {
    fn id(&self) -> &'static str {
        "timer"
    }

    fn title(&self) -> &'static str {
        "Timer"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<TimerState>();
        let me = self.me.clone();
        let on = move |f: fn(&Timer)| {
            let me = me.clone();
            move || {
                if let Some(t) = me.upgrade() {
                    f(&t)
                }
            }
        };
        g.on_start_pause(on(|t| t.update(|c, now| c.start_pause(now))));
        g.on_reset(on(|t| t.update(|c, _| c.reset())));
        let me = self.me.clone();
        g.on_add_minutes(move |n| {
            if let Some(t) = me.upgrade() {
                t.update(|c, _| c.add_minutes(n))
            }
        });
        let me = self.me.clone();
        g.on_set_stopwatch(move |sw| {
            if let Some(t) = me.upgrade() {
                t.update(|c, _| {
                    if c.stopwatch != sw {
                        *c = Clock {
                            stopwatch: sw,
                            duration: c.duration,
                            ..Clock::default()
                        };
                    }
                })
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn activity(&self) -> Option<super::Hud> {
        let c = self.clock.borrow();
        let now = Instant::now();
        if c.finished {
            return Some(super::Hud {
                icon: "timer",
                level: None,
                text: "Done".into(),
                warn: true,
            });
        }
        c.running().then(|| super::Hud {
            icon: "timer",
            // Countdown: remaining fraction drives the ring; stopwatch: plain icon.
            level: (!c.stopwatch)
                .then(|| c.shown(now).as_secs_f32() / c.duration.as_secs_f32().max(1.0)),
            text: format(c.shown(now)),
            warn: false,
        })
    }

    fn peek(&self) -> Option<String> {
        let c = self.clock.borrow();
        if c.finished {
            Some("Time's up".into())
        } else if c.running() {
            Some(format!("Timer  {}", format(c.shown(Instant::now()))))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format(Duration::from_secs(0)), "00:00");
        assert_eq!(format(Duration::from_secs(65)), "01:05");
        assert_eq!(format(Duration::from_secs(3600 + 61)), "1:01:01");
    }

    #[test]
    fn countdown_runs_pauses_and_finishes() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        c.start_pause(t0);
        assert_eq!(
            c.shown(t0 + Duration::from_secs(60)),
            Duration::from_secs(240)
        );
        c.start_pause(t0 + Duration::from_secs(60)); // pause
        assert_eq!(
            c.shown(t0 + Duration::from_secs(600)),
            Duration::from_secs(240)
        );
        c.start_pause(t0 + Duration::from_secs(600)); // resume
        let end = t0 + Duration::from_secs(600 + 240);
        assert!(c.check_finished(end));
        assert!(c.finished && !c.running());
        assert!(!c.check_finished(end), "fires once");
    }

    #[test]
    fn add_minutes_clamps() {
        let mut c = Clock::default();
        c.add_minutes(-10);
        assert_eq!(c.duration, Duration::from_secs(60));
        c.add_minutes(100_000);
        assert_eq!(c.duration, MAX);
    }

    #[test]
    fn stopwatch_counts_up() {
        let t0 = Instant::now();
        let mut c = Clock {
            stopwatch: true,
            ..Clock::default()
        };
        c.start_pause(t0);
        assert_eq!(c.shown(t0 + Duration::from_secs(7)), Duration::from_secs(7));
        assert!(!c.check_finished(t0 + Duration::from_secs(9999)));
    }
}
