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

        notch.ui.show()?;
        // The winit window exists only once the event loop runs.
        let weak = Rc::downgrade(&notch);
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(&weak, |n| n.apply(State::Collapsed, State::Collapsed))
        });
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
                with(&me, |n| n.handle(Event::LeaveTimeout))
            });
    }

    fn size_for(&self, s: State) -> (f32, f32) {
        let cfg = self.cfg.borrow();
        let collapsed = PILL;
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

/// Size the window to `w`x`h` logical px and center it at the top of its monitor.
fn place(ui: &NotchWindow, w: f32, h: f32) {
    let window = ui.window();
    window.set_size(slint::LogicalSize::new(w, h));
    let monitor =
        window.with_winit_window(|win| win.primary_monitor().or_else(|| win.current_monitor()));
    if let Some(Some(m)) = monitor {
        let scale = m.scale_factor() as f32;
        let x = m.position().x + ((m.size().width as f32 - w * scale) / 2.0) as i32;
        window.set_position(slint::PhysicalPosition::new(x, m.position().y));
    }
}

fn with(weak: &Weak<Notch>, f: impl FnOnce(&Notch)) {
    if let Some(n) = weak.upgrade() {
        f(&n)
    }
}
