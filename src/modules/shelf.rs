//! File shelf: drop files on the notch, copy them back (click) or drag them out (macOS).
//! Holds references only and lives in memory: nothing about the files is written to disk.

use super::{Ctx, Module};
use crate::{NotchWindow, ShelfEntry, ShelfState};
use clipboard_rs::{Clipboard as _, ClipboardContext};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, SharedPixelBuffer, VecModel};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

/// Larger images get no thumbnail (decoding would cost too much memory).
const THUMB_MAX_BYTES: u64 = 20 * 1024 * 1024;
const THUMB_PX: u32 = 64;
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

thread_local! {
    static INSTANCE: RefCell<Weak<Shelf>> = RefCell::default();
}

pub fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// "Downloads · 2.4 MB" / "Projects · folder"
pub fn describe(path: &Path, size: Option<u64>, is_dir: bool) -> String {
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let what = if is_dir {
        "folder".to_string()
    } else {
        size.map(|s| super::stats::format_bytes(s as f64))
            .unwrap_or_default()
    };
    [dir, what]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

struct Item {
    id: u64,
    path: PathBuf,
    size: Option<u64>,
    is_dir: bool,
    thumb: Option<slint::Image>,
}

pub struct Shelf {
    me: Weak<Shelf>,
    ctx: Ctx,
    items: RefCell<Vec<Item>>,
    next_id: Cell<u64>,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Shelf {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let s = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            items: RefCell::default(),
            next_id: Cell::new(1),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&s));
        s
    }

    fn add(&self, path: &Path) {
        if self.items.borrow().iter().any(|i| i.path == path) {
            return;
        }
        let meta = std::fs::metadata(path).ok();
        let id = self.next_id.replace(self.next_id.get() + 1);
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let size = meta.as_ref().filter(|m| m.is_file()).map(|m| m.len());
        self.items.borrow_mut().insert(
            0,
            Item {
                id,
                path: path.to_path_buf(),
                size,
                is_dir,
                thumb: None,
            },
        );
        if is_image(path) && size.is_some_and(|s| s <= THUMB_MAX_BYTES) {
            spawn_thumbnail(id, path.to_path_buf());
        }
    }

    fn path(&self, id: i32) -> Option<PathBuf> {
        self.items
            .borrow()
            .iter()
            .find(|i| i.id == id as u64)
            .map(|i| i.path.clone())
    }

    /// Path of an entry that still exists; drops it from the shelf if it's gone.
    fn existing(&self, id: i32) -> Option<PathBuf> {
        let p = self.path(id)?;
        if p.exists() {
            return Some(p);
        }
        self.items.borrow_mut().retain(|i| i.id != id as u64);
        self.sync();
        None
    }

    fn copy(&self, id: i32) {
        let Some(p) = self.existing(id) else { return };
        let r = ClipboardContext::new()
            .and_then(|c| c.set_files(vec![p.to_string_lossy().into_owned()]));
        match r {
            Ok(()) => (self.ctx.collapse)(),
            Err(e) => log::warn!("shelf: copy failed: {e}"),
        }
    }

    fn sync(&self) {
        let rows: Vec<ShelfEntry> = self
            .items
            .borrow()
            .iter()
            .map(|i| ShelfEntry {
                id: i.id as i32,
                name: i
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| i.path.display().to_string())
                    .into(),
                meta: describe(&i.path, i.size, i.is_dir).into(),
                has_thumb: i.thumb.is_some(),
                thumb: i.thumb.clone().unwrap_or_default(),
                is_dir: i.is_dir,
                missing: !i.path.exists(),
            })
            .collect();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<ShelfState>();
            g.set_entries(Rc::new(VecModel::from(rows.clone())).into());
            g.set_can_drag(cfg!(target_os = "macos"));
            true
        });
    }
}

/// Decode off the UI thread; the result hops back via the event loop.
fn spawn_thumbnail(id: u64, path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("thumbnail".into())
        .spawn(move || {
            let thumb = match image::open(&path) {
                Ok(img) => img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8(),
                Err(e) => return log::debug!("shelf: no thumbnail for {}: {e}", path.display()),
            };
            let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                thumb.as_raw(),
                thumb.width(),
                thumb.height(),
            );
            let _ = slint::invoke_from_event_loop(move || {
                let Some(s) = INSTANCE.with(|i| i.borrow().upgrade()) else {
                    return;
                };
                if let Some(item) = s.items.borrow_mut().iter_mut().find(|i| i.id == id) {
                    item.thumb = Some(slint::Image::from_rgba8(buf));
                }
                s.sync();
            });
        });
}

pub(super) fn reveal(p: &Path) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open")
        .arg(p.parent().unwrap_or(p))
        .spawn();
    if let Err(e) = r {
        log::warn!("shelf: reveal {}: {e}", p.display());
    }
}

pub(super) fn open(p: &Path) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(p).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p).spawn();
    if let Err(e) = r {
        log::warn!("shelf: open {}: {e}", p.display());
    }
}

impl Module for Shelf {
    fn id(&self) -> &'static str {
        "shelf"
    }

    fn title(&self) -> &'static str {
        "Shelf"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<ShelfState>();
        let on = |f: fn(&Shelf, i32)| {
            let me = self.me.clone();
            move |id: i32| {
                if let Some(s) = me.upgrade() {
                    f(&s, id)
                }
            }
        };
        g.on_copy(on(Shelf::copy));
        g.on_reveal(on(|s, id| {
            if let Some(p) = s.existing(id) {
                reveal(&p)
            }
        }));
        g.on_open(on(|s, id| {
            if let Some(p) = s.existing(id) {
                open(&p)
            }
        }));
        g.on_remove(on(|s, id| {
            s.items.borrow_mut().retain(|i| i.id != id as u64);
            s.sync();
        }));
        let me = self.me.clone();
        g.on_clear(move || {
            if let Some(s) = me.upgrade() {
                s.items.borrow_mut().clear();
                s.sync();
            }
        });
        let (me, win) = (self.me.clone(), ui.as_weak());
        g.on_drag(move |id| {
            let (Some(s), Some(ui)) = (me.upgrade(), win.upgrade()) else {
                return;
            };
            let Some(p) = s.existing(id) else { return };
            let started = ui
                .window()
                .with_winit_window(|w| crate::platform::start_file_drag(w, &[p]));
            log::debug!("shelf: drag started: {started:?}");
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }

    fn drop_file(&self, path: &Path) -> bool {
        self.add(path);
        self.sync();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_detection() {
        assert!(is_image(Path::new("/a/b/Photo.JPG")));
        assert!(is_image(Path::new("x.webp")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("png")));
    }

    #[test]
    fn descriptions() {
        assert_eq!(
            describe(
                Path::new("/Users/me/Downloads/a.zip"),
                Some(2_400_000),
                false
            ),
            "Downloads · 2.4 MB"
        );
        assert_eq!(
            describe(Path::new("/Users/me/Projects"), None, true),
            "me · folder"
        );
        assert_eq!(describe(Path::new("a.txt"), None, false), "");
    }
}
