//! Compiled-in modules. Adding one: a Rust type implementing `Module`, a `.slint` view
//! (plus one line in `ModuleView` in app.slint), and one line in `all()`.

mod calc;
mod clipboard;
mod stats;
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
    /// A key typed while the module is visible ("1", "+", "Enter", "Backspace", ...).
    /// Return true to consume it.
    fn key(&self, _key: &str) -> bool {
        false
    }
    /// Config loaded or changed.
    fn config_changed(&self, _cfg: &Config) {}
}

pub fn all(ctx: Ctx) -> Vec<Rc<dyn Module>> {
    vec![
        timer::Timer::new(ctx.clone()),
        calc::Calc::new(ctx.clone()),
        stats::Stats::new(ctx.clone()),
        clipboard::ClipboardModule::new(ctx),
    ]
}
