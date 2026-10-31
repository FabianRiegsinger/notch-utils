//! One notch window: feeds window events into the state machine and applies the
//! resulting state (window geometry + Slint properties).

use crate::config::Config;
use crate::state::{self, Event, State};
use crate::{NotchState, NotchWindow};
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
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
    leave_timer: slint::Timer,
    shrink_timer: slint::Timer,
}

impl Notch {
    pub fn new(cfg: Rc<RefCell<Config>>) -> Result<Rc<Self>, slint::PlatformError> {
        let ui = NotchWindow::new()?;
        let notch = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            ui,
            cfg,
            state: Cell::new(State::Collapsed),
            leave_timer: slint::Timer::default(),
            shrink_timer: slint::Timer::default(),
        });

        let weak = Rc::downgrade(&notch);
        notch
            .ui
            .on_clicked(move || with(&weak, |n| n.handle(Event::Click)));

        // Enter/leave come from the OS window, not a Slint TouchArea: the window always has
        // the shape's size, and child TouchAreas (module buttons) would steal hover otherwise.
        let weak = Rc::downgrade(&notch);
        notch.ui.window().on_winit_window_event(move |_, ev| {
            use winit::event::{ElementState, WindowEvent};
            use winit::keyboard::{Key, NamedKey};
            with(&weak, |n| match ev {
                WindowEvent::CursorEntered { .. } => {
                    n.leave_timer.stop();
                    n.handle(Event::PointerEnter);
                }
                WindowEvent::CursorLeft { .. } => n.start_leave_timer(),
                WindowEvent::Focused(false) => n.handle(Event::FocusLost),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed
                        && event.logical_key == Key::Named(NamedKey::Escape) =>
                {
                    n.handle(Event::Escape)
                }
                _ => {}
            });
            EventResult::Propagate
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
                    {
                        let s = n.state.get();
                        n.apply(s, s)
                    }
                }),
                Err(e) => log::error!("notch window was not created: {e}"),
            }
        })
        .map_err(|e| slint::PlatformError::Other(e.to_string()))?;
        Ok(notch)
    }

    pub fn handle(&self, event: Event) {
        let old = self.state.get();
        let new = state::next(old, event, self.cfg.borrow().behavior.open_mode);
        if new != old {
            log::debug!("{old:?} --{event:?}--> {new:?}");
            self.state.set(new);
            self.apply(old, new);
        }
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
        let collapsed = crate::platform::notch_size().unwrap_or(PILL);
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
        self.shrink_timer.stop();
        if w >= ow && h >= oh {
            // Grow: enlarge the (transparent) window first, then let the shape animate out.
            place(&self.ui, w, h);
        } else {
            // Shrink: animate the shape in, then shrink the window to match.
            let ui = self.ui.as_weak();
            self.shrink_timer
                .start(slint::TimerMode::SingleShot, SHRINK_AFTER, move || {
                    if let Some(ui) = ui.upgrade() {
                        place(&ui, w, h);
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

/// Size the window to `w`x`h` logical px and center it at the top of the screen.
fn place(ui: &NotchWindow, w: f32, h: f32) {
    // Tell Slint first: it resizes the OS window back to its own idea of the size otherwise.
    ui.window().set_size(slint::LogicalSize::new(w, h));
    ui.window()
        .with_winit_window(|win| crate::platform::place_top_center(win, w, h));
}

fn with(weak: &Weak<Notch>, f: impl FnOnce(&Notch)) {
    if let Some(n) = weak.upgrade() {
        f(&n)
    }
}
