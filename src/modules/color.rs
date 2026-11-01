//! Color picker: pick a color anywhere on screen (macOS NSColorSampler), show HEX / RGB / HSL
//! with copy buttons, and keep the last few picks in colors.json in the data dir.

use super::{Ctx, Module};
use crate::{ColorState, NotchWindow};
use slint::{ComponentHandle, VecModel};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

const RECENT: usize = 5;

type Rgb = [u8; 3];

fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn rgb(c: Rgb) -> String {
    format!("rgb({}, {}, {})", c[0], c[1], c[2])
}

/// CSS hsl() with rounded hue/saturation/lightness.
fn hsl(c: Rgb) -> String {
    let [r, g, b] = c.map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    format!("hsl({:.0}, {:.0}%, {:.0}%)", h, s * 100.0, l * 100.0)
}

fn store_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "notch-utils").map(|d| d.data_dir().join("colors.json"))
}

#[cfg(target_os = "macos")]
fn sample(done: impl Fn(Option<Rgb>) + 'static) {
    use block2::RcBlock;
    use objc2_app_kit::{NSColor, NSColorSampler, NSColorSpace};

    let handler = RcBlock::new(move |c: *mut NSColor| {
        // SAFETY: AppKit passes a valid NSColor or nil (cancelled), on the main thread.
        let picked = unsafe { c.as_ref() }.and_then(|c| {
            let c = c.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
            let v = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some([
                v(c.redComponent()),
                v(c.greenComponent()),
                v(c.blueComponent()),
            ])
        });
        done(picked);
    });
    let sampler = NSColorSampler::new();
    // SAFETY: the block lives as long as AppKit needs it (it retains it).
    unsafe { sampler.showSamplerWithSelectionHandler(&handler) };
}

#[cfg(not(target_os = "macos"))]
fn sample(done: impl Fn(Option<Rgb>) + 'static) {
    done(None)
}

pub struct ColorPicker {
    me: Weak<ColorPicker>,
    ctx: Ctx,
    /// Newest first; the first one is the current color.
    recent: RefCell<Vec<Rgb>>,
    copied: Cell<i32>,
    copied_timer: slint::Timer,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl ColorPicker {
    pub fn new(ctx: Ctx) -> Rc<Self> {
        let recent = store_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| vec![[0x0A, 0x6F, 0xE8]]);
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ctx,
            recent: RefCell::new(recent),
            copied: Cell::new(-1),
            copied_timer: slint::Timer::default(),
            windows: RefCell::default(),
        })
    }

    fn current(&self) -> Rgb {
        self.recent
            .borrow()
            .first()
            .copied()
            .unwrap_or([0x0A, 0x6F, 0xE8])
    }

    fn save(&self) {
        let Some(path) = store_path() else { return };
        let r = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| {
                std::fs::write(
                    &path,
                    serde_json::to_string(&*self.recent.borrow()).unwrap_or_default(),
                )
            });
        if let Err(e) = r {
            log::warn!("color: save failed: {e}");
        }
    }

    fn sync(&self) {
        let c = self.current();
        let slint_color = |c: Rgb| slint::Color::from_rgb_u8(c[0], c[1], c[2]);
        let recent: Vec<slint::Color> = self
            .recent
            .borrow()
            .iter()
            .map(|&c| slint_color(c))
            .collect();
        let copied = self.copied.get();
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<ColorState>();
            g.set_color(slint_color(c));
            g.set_hex(hex(c).into());
            g.set_rgb(rgb(c).into());
            g.set_hsl(hsl(c).into());
            g.set_recent(Rc::new(VecModel::from(recent.clone())).into());
            g.set_copied(copied);
            g.set_can_pick(cfg!(target_os = "macos"));
            true
        });
    }

    fn use_color(&self, c: Rgb) {
        let mut r = self.recent.borrow_mut();
        r.retain(|x| *x != c);
        r.insert(0, c);
        r.truncate(RECENT);
        drop(r);
        self.save();
        self.sync();
    }

    fn pick(&self) {
        // Keep the notch open while the pointer is out picking.
        (self.ctx.pin)(true);
        let me = self.me.clone();
        sample(move |picked| {
            let Some(m) = me.upgrade() else { return };
            (m.ctx.pin)(false);
            if let Some(c) = picked {
                m.use_color(c);
            }
        });
    }

    fn copy(&self, row: i32) {
        let c = self.current();
        let text = match row {
            0 => hex(c),
            1 => rgb(c),
            _ => hsl(c),
        };
        use clipboard_rs::{Clipboard, ClipboardContext};
        match ClipboardContext::new().and_then(|ctx| ctx.set_text(text)) {
            Ok(()) => {
                self.copied.set(row);
                let me = self.me.clone();
                self.copied_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(1200),
                    move || {
                        if let Some(m) = me.upgrade() {
                            m.copied.set(-1);
                            m.sync();
                        }
                    },
                );
            }
            Err(e) => log::warn!("color: copy failed: {e}"),
        }
        self.sync();
    }
}

impl Module for ColorPicker {
    fn id(&self) -> &'static str {
        "color"
    }

    fn title(&self) -> &'static str {
        "Color Picker"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<ColorState>();
        let me = self.me.clone();
        g.on_pick(move || {
            if let Some(m) = me.upgrade() {
                m.pick()
            }
        });
        let me = self.me.clone();
        g.on_copy(move |row| {
            if let Some(m) = me.upgrade() {
                m.copy(row)
            }
        });
        let me = self.me.clone();
        g.on_choose(move |i| {
            if let Some(m) = me.upgrade() {
                let c = m.recent.borrow().get(i as usize).copied();
                if let Some(c) = c {
                    m.use_color(c)
                }
            }
        });
        let me = self.me.clone();
        g.on_clear(move || {
            if let Some(m) = me.upgrade() {
                let c = m.current();
                *m.recent.borrow_mut() = vec![c];
                m.save();
                m.sync();
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
        self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let c = [10, 111, 232];
        assert_eq!(hex(c), "#0A6FE8");
        assert_eq!(rgb(c), "rgb(10, 111, 232)");
        assert_eq!(hsl(c), "hsl(213, 92%, 47%)");
        assert_eq!(hsl([128, 128, 128]), "hsl(0, 0%, 50%)");
        assert_eq!(hsl([255, 0, 0]), "hsl(0, 100%, 50%)");
    }
}
