//! One notch window: feeds window events into the state machine and applies the
//! resulting state (window geometry + Slint properties).

use crate::config::{Config, LayoutMode, Theme};
use crate::state::{self, Event, State};
use crate::{ModuleTab, NotchState, NotchWindow, Palette};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{Color, ComponentHandle, Model, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// Matches the `animate width, height` duration in app.slint, plus a small margin.
const SHRINK_AFTER: Duration = Duration::from_millis(270);
/// Collapsed size on screens without a hardware notch.
const PILL: (f32, f32) = (160.0, 24.0);

pub struct Notch {
    me: Weak<Notch>,
    ui: NotchWindow,
    cfg: Rc<RefCell<Config>>,
    state: Cell<State>,
    /// Pointer over the window (OS enter/leave), so a HUD doesn't collapse under it.
    hovered: Cell<bool>,
    /// `None` = main screen; `Some(i)` = platform screen index.
    screen: Cell<Option<usize>>,
    leave_timer: slint::Timer,
    shrink_timer: slint::Timer,
    /// Called after every state or tab change, so the app can update module visibility.
    on_change: Rc<dyn Fn()>,
    /// Offered every key press while expanded; true = a module consumed it.
    on_key: Rc<dyn Fn(&str) -> bool>,
}

impl Notch {
    pub fn new(
        cfg: Rc<RefCell<Config>>,
        on_change: Rc<dyn Fn()>,
        on_key: Rc<dyn Fn(&str) -> bool>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let ui = NotchWindow::new()?;
        let notch = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ui,
            cfg,
            state: Cell::new(State::Collapsed),
            hovered: Cell::new(false),
            screen: Cell::new(None),
            leave_timer: slint::Timer::default(),
            shrink_timer: slint::Timer::default(),
            on_change,
            on_key,
        });

        let weak = Rc::downgrade(&notch);
        notch
            .ui
            .on_clicked(move || with(&weak, |n| n.handle(Event::Click)));
        let weak = Rc::downgrade(&notch);
        notch
            .ui
            .on_tab_changed(move |_| with(&weak, |n| (n.on_change)()));

        // Enter/leave come from the OS window, not a Slint TouchArea: the window always has
        // the shape's size, and child TouchAreas (module buttons) would steal hover otherwise.
        let weak = Rc::downgrade(&notch);
        notch.ui.window().on_winit_window_event(move |_, ev| {
            use winit::event::{ElementState, WindowEvent};
            use winit::keyboard::{Key, NamedKey};
            let mut result = EventResult::Propagate;
            with(&weak, |n| match ev {
                WindowEvent::CursorEntered { .. } => {
                    n.hovered.set(true);
                    n.leave_timer.stop();
                    n.handle(Event::PointerEnter);
                }
                WindowEvent::CursorLeft { .. } => {
                    n.hovered.set(false);
                    n.start_leave_timer()
                }
                WindowEvent::Focused(false) => n.handle(Event::FocusLost),
                WindowEvent::ThemeChanged(_) => n.apply_appearance(),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && n.state.get() == State::Expanded =>
                {
                    let key = match &event.logical_key {
                        Key::Named(NamedKey::Enter) => Some("Enter"),
                        Key::Named(NamedKey::Backspace) => Some("Backspace"),
                        Key::Named(NamedKey::Delete) => Some("Delete"),
                        Key::Named(NamedKey::Escape) => Some("Escape"),
                        Key::Character(c) => Some(c.as_str()),
                        _ => None,
                    };
                    if let Some(key) = key {
                        if (n.on_key)(key) {
                            result = EventResult::PreventDefault;
                        } else if key == "Escape" {
                            n.handle(Event::Escape)
                        }
                    }
                }
                _ => {}
            });
            result
        });

        // macOS sets its own (higher) window level in platform::setup_window.
        notch.ui.set_on_top(!cfg!(target_os = "macos"));
        notch.ui.show()?;
        let weak = Rc::downgrade(&notch);
        let ui = notch.ui.clone_strong();
        slint::spawn_local(async move {
            match ui.window().winit_window().await {
                Ok(win) => with(&weak, |n| {
                    crate::platform::setup_window(&win);
                    n.apply_appearance();
                    let s = n.state.get();
                    n.apply(s, s);
                }),
                Err(e) => log::error!("notch window was not created: {e}"),
            }
        })
        .map_err(|e| slint::PlatformError::Other(e.to_string()))?;
        Ok(notch)
    }

    pub fn ui(&self) -> &NotchWindow {
        &self.ui
    }

    pub fn state(&self) -> State {
        self.state.get()
    }

    pub fn screen(&self) -> Option<usize> {
        self.screen.get()
    }

    pub fn set_screen(&self, screen: Option<usize>) {
        if self.screen.replace(screen) != screen {
            self.apply_appearance();
            let s = self.state.get();
            self.apply(s, s);
        }
    }

    pub fn screen_count(&self) -> usize {
        self.ui
            .window()
            .with_winit_window(crate::platform::screen_count)
            .unwrap_or(1)
    }

    pub fn cursor_screen(&self) -> Option<usize> {
        self.ui
            .window()
            .with_winit_window(crate::platform::cursor_screen)
            .flatten()
    }

    pub fn handle(&self, event: Event) {
        let old = self.state.get();
        let new = state::next(old, event, self.cfg.borrow().behavior.open_mode);
        if new != old {
            log::debug!("{old:?} --{event:?}--> {new:?}");
            self.state.set(new);
            self.apply(old, new);
            (self.on_change)();
        }
    }

    pub fn hovered(&self) -> bool {
        self.hovered.get()
    }

    /// Show (`Some`) or clear (`None`) the HUD in the peek area.
    pub fn set_hud(&self, hud: Option<&crate::modules::Hud>) {
        self.ui.set_hud_active(hud.is_some());
        if let Some(h) = hud {
            self.ui.set_hud_icon(h.icon.into());
            self.ui.set_hud_level(h.level.unwrap_or(-1.0));
            self.ui.set_hud_text(h.text.as_str().into());
            self.ui.set_hud_warn(h.warn);
        }
    }

    pub fn close(&self) {
        let _ = self.ui.hide();
    }

    pub fn set_peek_text(&self, text: &str) {
        if self.ui.get_peek_text() != text {
            self.ui.set_peek_text(text.into());
        }
    }

    /// Module ids currently on screen in this window.
    pub fn visible_modules(&self) -> Vec<String> {
        if self.state.get() != State::Expanded {
            return vec![];
        }
        let tabs = self.ui.get_modules();
        if self.ui.get_grid() {
            tabs.iter().map(|t| t.id.to_string()).collect()
        } else {
            let i =
                (self.ui.get_active_tab().max(0) as usize).min(tabs.row_count().saturating_sub(1));
            tabs.row_data(i)
                .map(|t| vec![t.id.to_string()])
                .unwrap_or_default()
        }
    }

    /// Re-read layout, appearance and size settings.
    pub fn apply_config(&self, tabs: Vec<ModuleTab>) {
        let count = tabs.len() as i32;
        self.ui.set_modules(Rc::new(VecModel::from(tabs)).into());
        if self.ui.get_active_tab() >= count {
            self.ui.set_active_tab(0);
        }
        self.ui
            .set_grid(self.cfg.borrow().layout.mode == LayoutMode::Grid);
        self.apply_appearance();
        let s = self.state.get();
        self.apply(s, s);
    }

    fn apply_appearance(&self) {
        let cfg = self.cfg.borrow();
        let light = cfg.appearance.theme == Theme::System
            && self.ui.window().with_winit_window(|w| w.theme()).flatten()
                == Some(winit::window::Theme::Light);
        let accent = match cfg.appearance.theme {
            Theme::Custom => parse_hex(&cfg.appearance.accent).unwrap_or_else(|| {
                log::warn!("invalid accent color {:?}", cfg.appearance.accent);
                DEFAULT_ACCENT
            }),
            _ => DEFAULT_ACCENT,
        };
        let (shape, fg) = if light {
            (
                Color::from_rgb_u8(0xF2, 0xF2, 0xF7),
                Color::from_rgb_u8(0, 0, 0),
            )
        } else {
            (
                Color::from_rgb_u8(0, 0, 0),
                Color::from_rgb_u8(0xFF, 0xFF, 0xFF),
            )
        };
        let p = self.ui.global::<Palette>();
        p.set_accent(accent);
        p.set_fg(fg);
        p.set_fg_dim(fg.with_alpha(0.6));
        p.set_surface(fg.with_alpha(0.08));
        p.set_surface_hover(fg.with_alpha(0.15));
        self.ui.set_shape_color(shape);
        let inset = crate::platform::notch_size(self.screen.get()).map_or(0.0, |(_, h)| h);
        self.ui.set_top_inset(inset);
    }

    fn start_leave_timer(&self) {
        let delay = Duration::from_millis(self.cfg.borrow().behavior.collapse_delay_ms);
        let me = self.me.clone();
        self.leave_timer
            .start(slint::TimerMode::SingleShot, delay, move || {
                with(&me, |n| {
                    let inside =
                        n.ui.window()
                            .with_winit_window(crate::platform::cursor_inside);
                    if inside.flatten() != Some(true) {
                        n.handle(Event::LeaveTimeout)
                    }
                })
            });
    }

    fn size_for(&self, s: State) -> (f32, f32) {
        let cfg = self.cfg.borrow();
        let collapsed = crate::platform::notch_size(self.screen.get()).unwrap_or(PILL);
        match s {
            State::Collapsed => collapsed,
            State::Peek => (360.0, (collapsed.1 + 12.0).max(48.0)),
            State::Expanded => (
                cfg.appearance.expanded_width as f32,
                cfg.appearance.expanded_height as f32,
            ),
        }
    }

    fn apply(&self, old: State, new: State) {
        let (ow, oh) = self.size_for(old);
        let (w, h) = self.size_for(new);
        self.ui.set_state(match new {
            State::Collapsed => NotchState::Collapsed,
            State::Peek => NotchState::Peek,
            State::Expanded => NotchState::Expanded,
        });
        // The window must hold the shape for the whole animation: grow it now to the larger
        // of both sizes, and shrink it to the final size once the animation is done.
        self.shrink_timer.stop();
        let screen = self.screen.get();
        place(&self.ui, screen, w.max(ow), h.max(oh));
        if w < ow || h < oh {
            let ui = self.ui.as_weak();
            self.shrink_timer
                .start(slint::TimerMode::SingleShot, SHRINK_AFTER, move || {
                    if let Some(ui) = ui.upgrade() {
                        place(&ui, screen, w, h);
                    }
                });
        }
        self.ui.set_shape_width(w);
        self.ui.set_shape_height(h);
        if new == State::Expanded {
            self.ui.window().with_winit_window(|w| w.focus_window());
        }
    }
}

const DEFAULT_ACCENT: Color = Color::from_rgb_u8(0x0A, 0x84, 0xFF);

/// `#RRGGBB` → color.
fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Size the window to `w`x`h` logical px and center it at the top of the screen.
fn place(ui: &NotchWindow, screen: Option<usize>, w: f32, h: f32) {
    // Tell Slint first: it resizes the OS window back to its own idea of the size otherwise.
    ui.window().set_size(slint::LogicalSize::new(w, h));
    ui.window()
        .with_winit_window(|win| crate::platform::place_top_center(win, screen, w, h));
}

fn with(weak: &Weak<Notch>, f: impl FnOnce(&Notch)) {
    if let Some(n) = weak.upgrade() {
        f(&n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors() {
        assert_eq!(
            parse_hex("#0A84FF"),
            Some(Color::from_rgb_u8(0x0A, 0x84, 0xFF))
        );
        assert_eq!(parse_hex("0A84FF"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
    }
}
