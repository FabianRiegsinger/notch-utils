//! Now Playing: current track, cover, progress and controls from the OS media session.
//! Backends: macOS MediaRemote bridge, Windows GSMTC, Linux playerctl. Runs only while enabled.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
use linux as backend;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(target_os = "windows")]
use windows as backend;

use super::{Ctx, Hud, Module};
use crate::config::Config;
use crate::{MusicState, NotchWindow};
use slint::{Color, ComponentHandle, SharedPixelBuffer};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum Artwork {
    Unchanged,
    Cleared,
    Data(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds.
    pub duration: Option<f64>,
    /// Seconds, as of when the snapshot was taken.
    pub elapsed: f64,
    pub playing: bool,
    pub artwork: Artwork,
}

#[derive(Debug, Clone)]
pub enum Update {
    Track(Snapshot),
    Empty,
    Unavailable(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Cmd {
    PlayPause,
    Next,
    Previous,
}

/// "1:06", "1:02:03"
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Average color of RGBA pixels, lifted a bit so the glow is visible on black.
pub fn glow_color(rgba: &[u8]) -> (u8, u8, u8) {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in rgba.as_chunks::<4>().0 {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
        n += 1;
    }
    if n == 0 {
        return (255, 55, 95);
    }
    let lift = |c: u64| ((c / n) as f32 * 1.25).min(255.0) as u8;
    (lift(r), lift(g), lift(b))
}

struct Cover {
    image: slint::Image,
    glow: Color,
}

fn decode_cover(bytes: &[u8]) -> Option<Cover> {
    let img = image::load_from_memory(bytes)
        .ok()?
        .thumbnail(256, 256)
        .to_rgba8();
    let small = image::imageops::thumbnail(&img, 16, 16);
    let (r, g, b) = glow_color(small.as_raw());
    let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.as_raw(),
        img.width(),
        img.height(),
    );
    Some(Cover {
        image: slint::Image::from_rgba8(buf),
        glow: Color::from_rgb_u8(r, g, b),
    })
}

thread_local! {
    static INSTANCE: RefCell<Weak<Music>> = RefCell::default();
}

pub struct Music {
    me: Weak<Music>,
    ctx: Ctx,
    enabled: Cell<bool>,
    track: RefCell<Option<Snapshot>>,
    /// When `track.elapsed` was measured.
    at: Cell<Instant>,
    cover: RefCell<Option<Cover>>,
    unavailable: RefCell<String>,
    watcher: RefCell<Option<backend::Watcher>>,
    visible: Cell<bool>,
    pulse: Cell<u32>,
    tick: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Music {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let m = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            enabled: Cell::new(false),
            track: RefCell::default(),
            at: Cell::new(Instant::now()),
            cover: RefCell::default(),
            unavailable: RefCell::default(),
            watcher: RefCell::default(),
            visible: Cell::new(false),
            pulse: Cell::new(0),
            tick: slint::Timer::default(),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&m));
        m
    }

    fn elapsed(&self) -> f64 {
        let t = self.track.borrow();
        let Some(t) = t.as_ref() else { return 0.0 };
        let e = t.elapsed
            + if t.playing {
                self.at.get().elapsed().as_secs_f64()
            } else {
                0.0
            };
        t.duration.map_or(e, |d| e.min(d))
    }

    fn update(&self, u: Update) {
        match u {
            Update::Track(mut s) => {
                match std::mem::replace(&mut s.artwork, Artwork::Unchanged) {
                    Artwork::Data(bytes) => *self.cover.borrow_mut() = decode_cover(&bytes),
                    Artwork::Cleared => *self.cover.borrow_mut() = None,
                    Artwork::Unchanged => {}
                }
                *self.track.borrow_mut() = Some(s);
                self.at.set(Instant::now());
                self.unavailable.borrow_mut().clear();
            }
            Update::Empty => {
                *self.track.borrow_mut() = None;
                *self.cover.borrow_mut() = None;
            }
            Update::Unavailable(why) => {
                log::warn!("now playing unavailable: {why}");
                *self.unavailable.borrow_mut() = why;
            }
        }
        self.retick();
        self.sync();
    }

    /// 1 s tick while something plays: progress text (when visible) and equalizer pulse.
    fn retick(&self) {
        let playing = self.track.borrow().as_ref().is_some_and(|t| t.playing);
        if playing && !self.tick.running() {
            let me = self.me.clone();
            self.tick.start(
                slint::TimerMode::Repeated,
                Duration::from_secs(1),
                move || {
                    if let Some(m) = me.upgrade() {
                        m.pulse.set(m.pulse.get().wrapping_add(1));
                        m.sync();
                    }
                },
            );
        } else if !playing {
            self.tick.stop();
        }
    }

    fn sync(&self) {
        let track = self.track.borrow().clone();
        let elapsed = self.elapsed();
        let cover = self.cover.borrow();
        let unavailable = self.unavailable.borrow().clone();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<MusicState>();
            g.set_has_track(track.is_some());
            g.set_unavailable(!unavailable.is_empty() && track.is_none());
            if let Some(t) = &track {
                g.set_title(t.title.as_str().into());
                g.set_artist(
                    if t.album.is_empty() {
                        t.artist.clone()
                    } else {
                        format!("{} — {}", t.artist, t.album)
                    }
                    .into(),
                );
                g.set_playing(t.playing);
                g.set_elapsed(clock(elapsed).into());
                g.set_remaining(
                    t.duration
                        .map_or(String::new(), |d| format!("-{}", clock(d - elapsed)))
                        .into(),
                );
                g.set_progress(t.duration.map_or(0.0, |d| (elapsed / d.max(1.0)) as f32));
            }
            g.set_has_cover(cover.is_some());
            if let Some(c) = cover.as_ref() {
                g.set_cover(c.image.clone());
                g.set_glow(c.glow);
            } else {
                g.set_glow(Color::from_rgb_u8(255, 55, 95));
            }
            true
        });
        (self.ctx.peek_changed)();
    }

    fn send(&self, cmd: Cmd) {
        backend::command(cmd);
        // Optimistic: flip play state right away; the OS confirms with the next update.
        if let Cmd::PlayPause = cmd
            && let Some(t) = self.track.borrow_mut().as_mut()
        {
            t.elapsed = self.elapsed();
            t.playing = !t.playing;
        }
        self.at.set(Instant::now());
        self.retick();
        self.sync();
    }
}

impl Module for Music {
    fn id(&self) -> &'static str {
        "music"
    }

    fn title(&self) -> &'static str {
        "Now Playing"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<MusicState>();
        let on = |cmd: Cmd| {
            let me = self.me.clone();
            move || {
                if let Some(m) = me.upgrade() {
                    m.send(cmd)
                }
            }
        };
        g.on_play_pause(on(Cmd::PlayPause));
        g.on_next(on(Cmd::Next));
        g.on_previous(on(Cmd::Previous));
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn on_show(&self) {
        self.visible.set(true);
        self.sync();
    }

    fn on_hide(&self) {
        self.visible.set(false);
    }

    fn config_changed(&self, cfg: &Config) {
        let on = cfg.layout.modules.iter().any(|m| m == "music");
        if self.enabled.replace(on) == on {
            return;
        }
        if on {
            *self.watcher.borrow_mut() = backend::watch(|u| {
                // Called on the backend thread.
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(m) = INSTANCE.with(|i| i.borrow().upgrade()) {
                        m.update(u)
                    }
                });
            });
            if self.watcher.borrow().is_none() {
                *self.unavailable.borrow_mut() = "no media source".into();
            }
        } else {
            self.watcher.borrow_mut().take();
            self.update(Update::Empty);
        }
        self.sync();
    }

    fn activity(&self) -> Option<Hud> {
        let t = self.track.borrow();
        let t = t.as_ref().filter(|t| t.playing)?;
        Some(Hud {
            icon: "music",
            text: t.title.clone(),
            image: self.cover.borrow().as_ref().map(|c| c.image.clone()),
            pulse: self.pulse.get(),
            ..Default::default()
        })
    }

    fn peek(&self) -> Option<String> {
        let t = self.track.borrow();
        let t = t.as_ref().filter(|t| t.playing)?;
        Some(if t.artist.is_empty() {
            t.title.clone()
        } else {
            format!("{} · {}", t.title, t.artist)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(66.4), "1:06");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(clock(-5.0), "0:00");
    }

    #[test]
    fn glow_is_lifted_average() {
        assert_eq!(
            glow_color(&[100, 0, 200, 255, 100, 0, 200, 255]),
            (125, 0, 250)
        );
        assert_eq!(glow_color(&[]), (255, 55, 95));
        assert_eq!(glow_color(&[250, 250, 250, 255]).0, 255, "clamped");
    }
}
