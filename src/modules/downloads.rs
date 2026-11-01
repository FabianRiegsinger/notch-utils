//! Downloads: watches the Downloads folder. Shows downloads in progress (browser partial files)
//! with speed, and the 10 newest finished files (open / show / copy / drag out).

use super::{Ctx, Hud, Module};
use crate::{DlEntry, DlState, NotchWindow};
use notify::{RecursiveMode, Watcher};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// In progress: the name the file will get when done.
    Partial(String),
    Done,
    /// Hidden and system files.
    Skip,
}

/// Chromium `.crdownload`, Firefox `.part`, Safari `.download` bundles, Opera `.opdownload`.
pub fn classify(name: &str) -> Kind {
    if name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini") {
        return Kind::Skip;
    }
    for ext in [".crdownload", ".part", ".download", ".opdownload"] {
        if let Some(stem) = name.strip_suffix(ext) {
            // Chromium names unconfirmed downloads "Unconfirmed 123.crdownload".
            return Kind::Partial(if stem.is_empty() {
                name.to_string()
            } else {
                stem.to_string()
            });
        }
    }
    Kind::Done
}

/// "just now", "3 min ago", "2 h ago", "yesterday", "4 days ago"
pub fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// Size of a file, or of a directory tree (Safari's .download bundles).
fn size_of(p: &Path) -> u64 {
    let Ok(m) = std::fs::metadata(p) else {
        return 0;
    };
    if m.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0)
    } else {
        m.len()
    }
}

struct Active {
    name: String,
    size: u64,
    speed: f64,
}

struct Finished {
    path: PathBuf,
    name: String,
    size: u64,
    modified: SystemTime,
}

pub struct Downloads {
    me: Weak<Downloads>,
    ctx: Ctx,
    dir: Option<PathBuf>,
    enabled: Cell<bool>,
    active: RefCell<Vec<Active>>,
    finished: RefCell<Vec<Finished>>,
    /// Sizes from the previous scan, for speed.
    last_sizes: RefCell<HashMap<String, (u64, Instant)>>,
    watcher: RefCell<Option<notify::RecommendedWatcher>>,
    debounce: slint::Timer,
    tick: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

thread_local! {
    static INSTANCE: RefCell<Weak<Downloads>> = RefCell::default();
}

impl Downloads {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let d = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            dir: directories::UserDirs::new().and_then(|u| u.download_dir().map(Path::to_path_buf)),
            enabled: Cell::new(false),
            active: RefCell::default(),
            finished: RefCell::default(),
            last_sizes: RefCell::default(),
            watcher: RefCell::default(),
            debounce: slint::Timer::default(),
            tick: slint::Timer::default(),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&d));
        d
    }

    fn scan(&self) {
        let Some(dir) = &self.dir else { return };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let now = Instant::now();
        let was_active: Vec<String> = self
            .active
            .borrow()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        let mut active = vec![];
        let mut finished = vec![];
        let mut sizes = HashMap::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            match classify(&name) {
                Kind::Skip => {}
                Kind::Partial(final_name) => {
                    let size = size_of(&e.path());
                    let speed = self
                        .last_sizes
                        .borrow()
                        .get(&name)
                        .map(|(s, t)| {
                            size.saturating_sub(*s) as f64
                                / now.duration_since(*t).as_secs_f64().max(0.1)
                        })
                        .unwrap_or(0.0);
                    sizes.insert(name, (size, now));
                    active.push(Active {
                        name: final_name,
                        size,
                        speed,
                    });
                }
                Kind::Done => {
                    let Ok(m) = e.metadata() else { continue };
                    finished.push(Finished {
                        path: e.path(),
                        name,
                        size: if m.is_dir() { 0 } else { m.len() },
                        modified: m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    });
                }
            }
        }
        finished.sort_by_key(|f| std::cmp::Reverse(f.modified));
        finished.truncate(10);
        // A download that vanished from "active" and whose final file exists just finished.
        for name in was_active {
            if !active.iter().any(|a| a.name == name) && finished.iter().any(|f| f.name == name) {
                (self.ctx.hud)(Hud {
                    icon: "download",
                    text: "Downloaded".into(),
                    ..Default::default()
                });
            }
        }
        *self.last_sizes.borrow_mut() = sizes;
        let downloading = !active.is_empty();
        *self.active.borrow_mut() = active;
        *self.finished.borrow_mut() = finished;
        // Re-scan every second while something downloads (size and speed).
        if downloading && !self.tick.running() {
            let me = self.me.clone();
            self.tick.start(
                slint::TimerMode::Repeated,
                Duration::from_secs(1),
                move || {
                    if let Some(d) = me.upgrade() {
                        d.scan()
                    }
                },
            );
        } else if !downloading {
            self.tick.stop();
        }
        self.sync();
    }

    fn sync(&self) {
        let fmt = super::stats::format_bytes;
        let now = SystemTime::now();
        let rows: Vec<DlEntry> = self
            .active
            .borrow()
            .iter()
            .map(|a| DlEntry {
                name: a.name.as_str().into(),
                meta: format!("{} · {}/s", fmt(a.size as f64), fmt(a.speed)).into(),
                active: true,
                index: -1,
            })
            .chain(self.finished.borrow().iter().enumerate().map(|(i, f)| {
                DlEntry {
                    name: f.name.as_str().into(),
                    meta: {
                        let age = ago(now
                            .duration_since(f.modified)
                            .map(|d| d.as_secs())
                            .unwrap_or(0));
                        if f.size > 0 {
                            format!("{} · {age}", fmt(f.size as f64))
                        } else {
                            age
                        }
                    }
                    .into(),
                    active: false,
                    index: i as i32,
                }
            }))
            .collect();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<DlState>();
            g.set_entries(Rc::new(VecModel::from(rows.clone())).into());
            g.set_available(self.dir.is_some());
            g.set_can_drag(cfg!(target_os = "macos"));
            true
        });
        (self.ctx.peek_changed)();
    }

    fn path(&self, i: i32) -> Option<PathBuf> {
        self.finished
            .borrow()
            .get(i.max(0) as usize)
            .map(|f| f.path.clone())
    }

    fn start(&self) {
        let Some(dir) = &self.dir else { return };
        let w = notify::recommended_watcher(|res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = slint::invoke_from_event_loop(|| {
                    let Some(d) = INSTANCE.with(|i| i.borrow().upgrade()) else {
                        return;
                    };
                    let me = d.me.clone();
                    // Coalesce bursts (a download writes thousands of times).
                    d.debounce.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(400),
                        move || {
                            if let Some(d) = me.upgrade() {
                                d.scan()
                            }
                        },
                    );
                });
            }
        });
        match w {
            Ok(mut w) => match w.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => *self.watcher.borrow_mut() = Some(w),
                Err(e) => log::warn!("downloads: watching {}: {e}", dir.display()),
            },
            Err(e) => log::warn!("downloads: {e}"),
        }
        self.scan();
    }
}

impl Module for Downloads {
    fn id(&self) -> &'static str {
        "downloads"
    }

    fn title(&self) -> &'static str {
        "Downloads"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<DlState>();
        let on = |f: fn(&Path)| {
            let me = self.me.clone();
            move |i: i32| {
                if let Some(p) = me.upgrade().and_then(|d| d.path(i)) {
                    f(&p)
                }
            }
        };
        g.on_open(on(super::shelf::open));
        g.on_reveal(on(super::shelf::reveal));
        let (me, win) = (self.me.clone(), ui.as_weak());
        g.on_drag(move |i| {
            let (Some(d), Some(ui)) = (me.upgrade(), win.upgrade()) else {
                return;
            };
            if let Some(p) = d.path(i) {
                ui.window()
                    .with_winit_window(|w| crate::platform::start_file_drag(w, &[p]));
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn on_show(&self) {
        // Ages ("3 min ago") move on; refresh when opened.
        self.sync();
    }

    fn config_changed(&self, cfg: &crate::config::Config) {
        let on = cfg.layout.modules.iter().any(|m| m == "downloads");
        if self.enabled.replace(on) == on {
            return;
        }
        if on {
            self.start();
        } else {
            self.watcher.borrow_mut().take();
            self.tick.stop();
            self.active.borrow_mut().clear();
            self.finished.borrow_mut().clear();
            self.sync();
        }
    }

    fn activity(&self) -> Option<Hud> {
        let a = self.active.borrow();
        let speed: f64 = a.iter().map(|a| a.speed).sum();
        (!a.is_empty()).then(|| Hud {
            icon: "download",
            text: format!("{}/s", super::stats::format_bytes(speed)).replace(" ", ""),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_browser_partials() {
        assert_eq!(
            classify("movie.mkv.crdownload"),
            Kind::Partial("movie.mkv".into())
        );
        assert_eq!(
            classify("setup.exe.part"),
            Kind::Partial("setup.exe".into())
        );
        assert_eq!(
            classify("Photo.jpg.download"),
            Kind::Partial("Photo.jpg".into())
        );
        assert_eq!(classify("report.pdf"), Kind::Done);
        assert_eq!(classify(".DS_Store"), Kind::Skip);
        assert_eq!(classify(".crdownload"), Kind::Skip, "hidden");
    }

    #[test]
    fn ages() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(180), "3 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(90_000), "yesterday");
        assert_eq!(ago(4 * 86_400), "4 days ago");
    }
}
