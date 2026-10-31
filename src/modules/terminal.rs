//! A real terminal in the notch: alacritty_terminal for VT emulation, its tty module for the
//! PTY (Unix PTY / Windows ConPTY) and its PTY event loop thread. The shell starts on first
//! show and keeps running while the notch is closed. Rendering is rows of style runs.

use super::{Ctx, KeyInput, Module};
use crate::{NotchWindow, TermLine, TermRun, TermState};
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as VtColor, CursorShape, NamedColor, Rgb};
use slint::{Color, ComponentHandle, VecModel};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const SCROLLBACK: usize = 2000;
const FG: (u8, u8, u8) = (0xE6, 0xE6, 0xE6);

thread_local! {
    static INSTANCE: RefCell<Weak<Terminal>> = RefCell::default();
}

// ---------- pure helpers ----------

/// xterm 256-color palette.
pub fn xterm(i: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x1d, 0x1f, 0x21),
        (0xcc, 0x66, 0x66),
        (0xb5, 0xbd, 0x68),
        (0xf0, 0xc6, 0x74),
        (0x81, 0xa2, 0xbe),
        (0xb2, 0x94, 0xbb),
        (0x8a, 0xbe, 0xb7),
        (0xc5, 0xc8, 0xc6),
        (0x66, 0x66, 0x66),
        (0xd5, 0x4e, 0x53),
        (0xb9, 0xca, 0x4a),
        (0xe7, 0xc5, 0x47),
        (0x7a, 0xa6, 0xda),
        (0xc3, 0x97, 0xd8),
        (0x70, 0xc0, 0xb1),
        (0xea, 0xea, 0xea),
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
            (v(i / 36), v(i / 6 % 6), v(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            (g, g, g)
        }
    }
}

/// Resolved color, or None for the default background (drawn transparent over the notch).
fn resolve(
    c: VtColor,
    overrides: &alacritty_terminal::term::color::Colors,
    dim: bool,
) -> Option<(u8, u8, u8)> {
    let rgb = |r: Rgb| (r.r, r.g, r.b);
    let idx = |i: usize, fallback: (u8, u8, u8)| overrides[i].map(rgb).unwrap_or(fallback);
    let base = match c {
        VtColor::Spec(r) => rgb(r),
        VtColor::Indexed(i) => idx(i as usize, xterm(i)),
        VtColor::Named(n) => match n {
            NamedColor::Background => return overrides[n as usize].map(rgb),
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => {
                idx(n as usize, FG)
            }
            NamedColor::DimForeground => idx(n as usize, (0x99, 0x99, 0x99)),
            n if (n as usize) < 16 => idx(n as usize, xterm(n as u8)),
            // Dim variants of the 8 base colors.
            n => {
                let i = (n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8;
                let (r, g, b) = xterm(i as u8);
                (r * 2 / 3, g * 2 / 3, b * 2 / 3)
            }
        },
    };
    Some(if dim {
        (base.0 * 2 / 3, base.1 * 2 / 3, base.2 * 2 / 3)
    } else {
        base
    })
}

/// Bytes for a key press, or None if the terminal doesn't use it.
pub fn encode_key(k: &KeyInput, app_cursor: bool) -> Option<Vec<u8>> {
    let modbits = 1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8;
    let csi = |tail: &str| -> Vec<u8> {
        if modbits > 1 {
            // "\x1b[1;5A" style for modified cursor keys.
            let (num, letter) = tail.split_at(tail.len() - 1);
            let num = if num.is_empty() {
                "1"
            } else {
                num.trim_end_matches('~')
            };
            format!("\x1b[{num};{modbits}{letter}").into_bytes()
        } else {
            format!("\x1b[{tail}").into_bytes()
        }
    };
    let ss3_or_csi = |c: char| -> Vec<u8> {
        if app_cursor && modbits == 1 {
            format!("\x1bO{c}").into_bytes()
        } else {
            csi(&c.to_string())
        }
    };
    if k.named {
        return Some(match k.key.as_str() {
            "Enter" => b"\r".to_vec(),
            "Backspace" if k.ctrl => vec![0x08],
            "Backspace" => vec![0x7f],
            "Tab" if k.shift => b"\x1b[Z".to_vec(),
            "Tab" => b"\t".to_vec(),
            "Escape" => vec![0x1b],
            "Space" if k.ctrl => vec![0],
            "Space" => b" ".to_vec(),
            "ArrowUp" => ss3_or_csi('A'),
            "ArrowDown" => ss3_or_csi('B'),
            "ArrowRight" => ss3_or_csi('C'),
            "ArrowLeft" => ss3_or_csi('D'),
            "Home" => ss3_or_csi('H'),
            "End" => ss3_or_csi('F'),
            "PageUp" => b"\x1b[5~".to_vec(),
            "PageDown" => b"\x1b[6~".to_vec(),
            "Delete" => b"\x1b[3~".to_vec(),
            "Insert" => b"\x1b[2~".to_vec(),
            "F1" => b"\x1bOP".to_vec(),
            "F2" => b"\x1bOQ".to_vec(),
            "F3" => b"\x1bOR".to_vec(),
            "F4" => b"\x1bOS".to_vec(),
            f if f.starts_with('F') => {
                let n: u8 = f[1..].parse().ok()?;
                let code = match n {
                    5 => 15,
                    6..=10 => n + 11,
                    11 | 12 => n + 12,
                    _ => return None,
                };
                format!("\x1b[{code}~").into_bytes()
            }
            _ => return None,
        });
    }
    if k.logo {
        return None; // Cmd shortcuts are the app's (paste is handled separately).
    }
    let mut out = vec![];
    // macOS: Option composes characters (é, @, …), as in Terminal.app; elsewhere Alt = ESC prefix.
    if k.alt && !cfg!(target_os = "macos") {
        out.push(0x1b);
    }
    match (k.ctrl, k.key.as_bytes()) {
        (true, [c]) if c.is_ascii_alphabetic() || b"@[\\]^_".contains(c) => {
            out.push(c.to_ascii_uppercase() & 0x1f)
        }
        (true, b"?") => out.push(0x7f),
        _ => out.extend_from_slice(k.key.as_bytes()),
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: (u8, u8, u8),
    /// None = default background (transparent).
    bg: Option<(u8, u8, u8)>,
    bold: bool,
}

/// Where the current style run ends, to decide whether the next cell extends it.
#[derive(Clone, Copy)]
struct RunEnd {
    row: usize,
    next_col: usize,
    style: Style,
}

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ---------- session ----------

#[derive(Clone)]
struct Listener {
    sender: Arc<Mutex<Option<EventLoopSender>>>,
    /// A render is already queued on the UI thread: coalesces bursts of output.
    pending: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
}

impl EventListener for Listener {
    fn send_event(&self, ev: TermEvent) {
        match ev {
            TermEvent::Wakeup => {
                if !self.pending.swap(true, Ordering::AcqRel) {
                    let pending = self.pending.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        pending.store(false, Ordering::Release);
                        if let Some(t) = INSTANCE.with(|i| i.borrow().upgrade()) {
                            t.render();
                        }
                    });
                }
            }
            // Replies the terminal owes the program (cursor position reports etc.).
            TermEvent::PtyWrite(s) => {
                if let Some(tx) = self.sender.lock().ok().and_then(|g| g.clone()) {
                    let _ = tx.send(Msg::Input(Cow::Owned(s.into_bytes())));
                }
            }
            TermEvent::ChildExit(_) | TermEvent::Exit => {
                self.exited.store(true, Ordering::Release);
                let _ = slint::invoke_from_event_loop(|| {
                    if let Some(t) = INSTANCE.with(|i| i.borrow().upgrade()) {
                        t.render();
                    }
                });
            }
            _ => {}
        }
    }
}

struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    tx: EventLoopSender,
    exited: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

pub struct Terminal {
    me: Weak<Terminal>,
    session: RefCell<Option<Session>>,
    size: Cell<(usize, usize)>,
    cell: Cell<(f32, f32)>,
    visible: Cell<bool>,
    windows: RefCell<Vec<slint::Weak<NotchWindow>>>,
}

impl Terminal {
    pub fn new(_ctx: Ctx) -> Rc<Self> {
        let t = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            session: RefCell::default(),
            size: Cell::new((80, 15)),
            cell: Cell::new((7.0, 15.0)),
            visible: Cell::new(false),
            windows: RefCell::default(),
        });
        INSTANCE.with(|i| *i.borrow_mut() = Rc::downgrade(&t));
        t
    }

    fn window_size(&self) -> WindowSize {
        let ((cols, rows), (cw, ch)) = (self.size.get(), self.cell.get());
        WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: cw as u16,
            cell_height: ch as u16,
        }
    }

    fn start(&self) {
        tty::setup_env();
        let opts = tty::Options {
            working_directory: directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()),
            ..Default::default()
        };
        let (cols, rows) = self.size.get();
        let listener = Listener {
            sender: Arc::default(),
            pending: Arc::default(),
            exited: Arc::default(),
        };
        let pty = match tty::new(&opts, self.window_size(), 0) {
            Ok(p) => p,
            Err(e) => return log::error!("terminal: could not start shell: {e}"),
        };
        let config = TermConfig {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &Size { cols, rows },
            listener.clone(),
        )));
        let ev = match EventLoop::new(term.clone(), listener.clone(), pty, false, false) {
            Ok(ev) => ev,
            Err(e) => return log::error!("terminal: event loop: {e}"),
        };
        let tx = ev.channel();
        if let Ok(mut s) = listener.sender.lock() {
            *s = Some(tx.clone());
        }
        ev.spawn();
        *self.session.borrow_mut() = Some(Session {
            term,
            tx,
            exited: listener.exited,
        });
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(s) = &*self.session.borrow() {
            s.term.lock().scroll_display(Scroll::Bottom);
            let _ = s.tx.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    fn paste(&self) {
        use clipboard_rs::{Clipboard as _, ClipboardContext};
        let Some(text) = ClipboardContext::new().ok().and_then(|c| c.get_text().ok()) else {
            return;
        };
        let bracketed = self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.term.lock().mode().contains(TermMode::BRACKETED_PASTE));
        // Strip ESC so pasted text can't end bracketed paste early or inject sequences.
        let text = text
            .replace('\x1b', "")
            .replace("\r\n", "\r")
            .replace('\n', "\r");
        let bytes = if bracketed {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text
        };
        self.write(bytes.into_bytes());
    }

    fn resize(&self, w: f32, h: f32) {
        let (cw, ch) = self.cell.get();
        let cols = ((w / cw).floor() as usize).max(10);
        let rows = ((h / ch).floor() as usize).max(3);
        if self.size.replace((cols, rows)) == (cols, rows) {
            return;
        }
        if let Some(s) = &*self.session.borrow() {
            s.term.lock().resize(Size { cols, rows });
            let _ = s.tx.send(Msg::Resize(self.window_size()));
        }
        self.render();
    }

    fn render(&self) {
        if !self.visible.get() {
            return;
        }
        let session = self.session.borrow();
        let Some(s) = session.as_ref() else { return };
        let exited = s.exited.load(Ordering::Acquire);
        let (lines, cursor) = {
            let term = s.term.lock();
            let content = term.renderable_content();
            let offset = content.display_offset as i32;
            let rows = term.screen_lines();
            let mut lines: Vec<Vec<TermRun>> = vec![vec![]; rows];
            let mut last: Option<RunEnd> = None;
            for cell in content.display_iter {
                let row = (cell.point.line.0 + offset) as usize;
                let col = cell.point.column.0;
                if row >= rows
                    || cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let dim = cell.flags.contains(Flags::DIM);
                let (mut fg, mut bg) = (
                    resolve(cell.fg, content.colors, dim).unwrap_or(FG),
                    resolve(cell.bg, content.colors, false),
                );
                if cell.flags.contains(Flags::INVERSE) {
                    (fg, bg) = (bg.unwrap_or((0, 0, 0)), Some(fg));
                }
                let ch = if cell.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    cell.c
                };
                let bold = cell.flags.contains(Flags::BOLD);
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let line = &mut lines[row];
                // Extend the previous run when the style matches and the cell is adjacent.
                let style = Style { fg, bg, bold };
                match (last, line.last_mut()) {
                    (Some(end), Some(run))
                        if end.row == row && end.next_col == col && end.style == style && !wide =>
                    {
                        let mut t = run.text.to_string();
                        t.push(ch);
                        run.text = t.into();
                    }
                    _ => line.push(TermRun {
                        col: col as i32,
                        text: ch.to_string().into(),
                        fg: Color::from_rgb_u8(fg.0, fg.1, fg.2),
                        bg: bg.map_or(Color::default(), |b| Color::from_rgb_u8(b.0, b.1, b.2)),
                        has_bg: bg.is_some(),
                        bold,
                    }),
                }
                last = Some(RunEnd {
                    row,
                    next_col: col + if wide { 2 } else { 1 },
                    style,
                });
            }
            // Drop runs that are only default-background spaces: fewer items to lay out.
            for line in &mut lines {
                line.retain(|r| r.has_bg || !r.text.trim().is_empty());
            }
            let c = content.cursor;
            let cursor = (c.shape != CursorShape::Hidden
                && content.mode.contains(TermMode::SHOW_CURSOR))
            .then(|| (c.point.column.0 as i32, c.point.line.0 + offset));
            (lines, cursor)
        };
        let model: Vec<TermLine> = lines
            .into_iter()
            .map(|runs| TermLine {
                runs: Rc::new(VecModel::from(runs)).into(),
            })
            .collect();
        let model = Rc::new(VecModel::from(model));
        self.windows.borrow_mut().retain(|w| {
            let Some(ui) = w.upgrade() else { return false };
            let g = ui.global::<TermState>();
            g.set_lines(model.clone().into());
            g.set_cursor_visible(cursor.is_some());
            if let Some((c, r)) = cursor {
                g.set_cursor_col(c);
                g.set_cursor_row(r);
            }
            g.set_exited(exited);
            true
        });
    }
}

impl Module for Terminal {
    fn id(&self) -> &'static str {
        "terminal"
    }

    fn title(&self) -> &'static str {
        "Terminal"
    }

    fn attach(&self, ui: &NotchWindow) {
        let g = ui.global::<TermState>();
        g.set_font_family(
            if cfg!(target_os = "macos") {
                "Menlo"
            } else if cfg!(windows) {
                "Consolas"
            } else {
                "DejaVu Sans Mono"
            }
            .into(),
        );
        let me = self.me.clone();
        g.on_resized(move |w, h, cw, ch| {
            if let Some(t) = me.upgrade() {
                if cw > 0.0 && ch > 0.0 {
                    t.cell.set((cw, ch));
                }
                t.resize(w, h);
            }
        });
        let me = self.me.clone();
        g.on_scroll(move |lines| {
            if let Some(t) = me.upgrade() {
                if let Some(s) = &*t.session.borrow() {
                    s.term.lock().scroll_display(Scroll::Delta(lines));
                }
                t.render();
            }
        });
        self.windows.borrow_mut().push(ui.as_weak());
    }

    fn on_show(&self) {
        self.visible.set(true);
        if self.session.borrow().is_none() {
            self.start();
        }
        self.render();
    }

    fn on_hide(&self) {
        self.visible.set(false);
    }

    fn key(&self, k: &KeyInput) -> bool {
        // Restart after the shell exited.
        let exited = self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.exited.load(Ordering::Acquire));
        if exited {
            if k.named && k.key == "Enter" {
                self.session.borrow_mut().take();
                self.start();
                self.render();
            }
            return true;
        }
        let paste = if cfg!(target_os = "macos") {
            k.logo && k.key == "v"
        } else {
            k.ctrl && k.shift && k.key.eq_ignore_ascii_case("v")
        };
        if paste {
            self.paste();
            return true;
        }
        let app_cursor = self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.term.lock().mode().contains(TermMode::APP_CURSOR));
        match encode_key(k, app_cursor) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str, named: bool) -> KeyInput {
        KeyInput {
            key: k.into(),
            named,
            ..Default::default()
        }
    }

    #[test]
    fn plain_and_named_keys() {
        assert_eq!(encode_key(&key("a", false), false), Some(b"a".to_vec()));
        assert_eq!(encode_key(&key("Enter", true), false), Some(b"\r".to_vec()));
        assert_eq!(encode_key(&key("Backspace", true), false), Some(vec![0x7f]));
        assert_eq!(encode_key(&key("Escape", true), false), Some(vec![0x1b]));
        assert_eq!(
            encode_key(&key("F5", true), false),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            encode_key(&key("F12", true), false),
            Some(b"\x1b[24~".to_vec())
        );
        assert_eq!(encode_key(&key("Shift", true), false), None);
    }

    #[test]
    fn cursor_keys_follow_app_cursor_mode_and_modifiers() {
        assert_eq!(
            encode_key(&key("ArrowUp", true), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key("ArrowUp", true), true),
            Some(b"\x1bOA".to_vec())
        );
        let ctrl_left = KeyInput {
            ctrl: true,
            ..key("ArrowLeft", true)
        };
        assert_eq!(encode_key(&ctrl_left, true), Some(b"\x1b[1;5D".to_vec()));
        let shift_tab = KeyInput {
            shift: true,
            ..key("Tab", true)
        };
        assert_eq!(encode_key(&shift_tab, false), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn ctrl_and_cmd() {
        let ctrl_c = KeyInput {
            ctrl: true,
            ..key("c", false)
        };
        assert_eq!(encode_key(&ctrl_c, false), Some(vec![3]));
        let ctrl_bracket = KeyInput {
            ctrl: true,
            ..key("[", false)
        };
        assert_eq!(encode_key(&ctrl_bracket, false), Some(vec![0x1b]));
        let cmd_k = KeyInput {
            logo: true,
            ..key("k", false)
        };
        assert_eq!(encode_key(&cmd_k, false), None);
    }

    #[test]
    fn palette() {
        assert_eq!(xterm(16), (0, 0, 0));
        assert_eq!(xterm(231), (255, 255, 255));
        assert_eq!(xterm(232), (8, 8, 8));
        assert_eq!(xterm(255), (238, 238, 238));
    }
}
