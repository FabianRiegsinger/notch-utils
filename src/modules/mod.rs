//! Compiled-in modules. Adding one: a Rust type implementing `Module`, a `.slint` view
//! (plus one line in `ModuleView` in app.slint), and one line in `all()`.

mod ai;
mod calc;
mod calendar;
mod clipboard;
mod hud;
mod shelf;
mod stats;
mod terminal;
mod timer;

use crate::NotchWindow;
use crate::config::Config;
use std::rc::Rc;

/// How a module talks back to the app.
#[derive(Clone)]
pub struct Ctx {
    /// The module's peek item changed.
    pub peek_changed: Rc<dyn Fn()>,
    /// The module wants attention: collapsed notches open to peek.
    pub alert: Rc<dyn Fn()>,
    /// Close the notch (e.g. after copying, so the user can paste).
    pub collapse: Rc<dyn Fn()>,
    /// Flash a short HUD in the peek area (volume, battery).
    pub hud: Rc<dyn Fn(Hud)>,
}

/// A key press: either typed text ("a", "+", "ü") or a named key ("Enter", "ArrowUp", "F5").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyInput {
    pub key: String,
    pub named: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS, Win key elsewhere.
    pub logo: bool,
}

impl KeyInput {
    /// The key for simple consumers: plain text or a named key, without Ctrl/Alt/Cmd.
    pub fn plain(&self) -> Option<&str> {
        (!self.ctrl && !self.alt && !self.logo).then_some(self.key.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hud {
    pub icon: &'static str,
    /// 0..1 shows a bar.
    pub level: Option<f32>,
    pub text: String,
    /// Shown in red.
    pub warn: bool,
}

pub trait Module {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    /// Bind the module's Slint global on a new notch window. Called once per window.
    fn attach(&self, ui: &NotchWindow);
    /// The module became visible in at least one window: start UI-only timers.
    fn on_show(&self) {}
    /// No window shows the module any more: stop UI-only timers.
    fn on_hide(&self) {}
    /// Short status for the peek state, e.g. a running timer.
    fn peek(&self) -> Option<String> {
        None
    }
    /// A key pressed while the module is visible. Return true to consume it.
    fn key(&self, _key: &KeyInput) -> bool {
        false
    }
    /// Config loaded or changed.
    fn config_changed(&self, _cfg: &Config) {}
    /// Data pushed from outside (IPC), e.g. topic "usage".
    fn receive(&self, _topic: &str, _payload: &str) {}
    /// A file was dropped on the notch. Return true to take it.
    fn drop_file(&self, _path: &std::path::Path) -> bool {
        false
    }
    /// False for modules that only show in the peek (no tab).
    fn has_view(&self) -> bool {
        true
    }
}

pub fn all(ctx: Ctx) -> Vec<Rc<dyn Module>> {
    vec![
        timer::Timer::new(ctx.clone()),
        calc::Calc::new(ctx.clone()),
        stats::Stats::new(ctx.clone()),
        clipboard::ClipboardModule::new(ctx.clone()),
        shelf::Shelf::new(ctx.clone()),
        hud::HudModule::new(ctx.clone()),
        ai::AiUsage::new(ctx.clone()),
        calendar::CalendarModule::new(ctx.clone()),
        terminal::Terminal::new(ctx),
    ]
}
