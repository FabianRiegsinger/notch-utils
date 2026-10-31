//! Owns the notch windows and the modules, and routes between them.

use crate::ModuleTab;
use crate::config::Config;
use crate::modules::{self, Module};
use crate::notch::Notch;
use crate::state::{Event, State};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

pub struct App {
    me: Weak<App>,
    cfg: Rc<RefCell<Config>>,
    modules: Vec<Rc<dyn Module>>,
    notches: RefCell<Vec<Rc<Notch>>>,
    /// Module ids visible in at least one window.
    shown: RefCell<HashSet<String>>,
}

impl App {
    pub fn new(cfg: Rc<RefCell<Config>>) -> Result<Rc<Self>, slint::PlatformError> {
        let app = Rc::new_cyclic(|me: &Weak<App>| {
            let (a, b) = (me.clone(), me.clone());
            let ctx = modules::Ctx {
                peek_changed: Rc::new(move || {
                    if let Some(app) = a.upgrade() {
                        app.refresh_peek()
                    }
                }),
                alert: Rc::new(move || {
                    if let Some(app) = b.upgrade() {
                        app.alert()
                    }
                }),
            };
            App {
                me: me.clone(),
                cfg,
                modules: modules::all(ctx),
                notches: RefCell::default(),
                shown: RefCell::default(),
            }
        });
        app.add_notch()?;
        app.apply_config();
        Ok(app)
    }

    fn add_notch(&self) -> Result<(), slint::PlatformError> {
        let me = self.me.clone();
        let notch = Notch::new(
            self.cfg.clone(),
            Rc::new(move || {
                if let Some(app) = me.upgrade() {
                    app.update_visibility()
                }
            }),
        )?;
        for m in &self.modules {
            m.attach(notch.ui());
        }
        self.notches.borrow_mut().push(notch);
        Ok(())
    }

    /// Enabled modules in configured order; unknown ids are skipped with a warning.
    fn enabled(&self) -> Vec<Rc<dyn Module>> {
        self.cfg
            .borrow()
            .layout
            .modules
            .iter()
            .filter_map(|id| {
                let m = self.modules.iter().find(|m| m.id() == id);
                if m.is_none() {
                    log::warn!("unknown module {id:?} in config");
                }
                m.cloned()
            })
            .collect()
    }

    pub fn apply_config(&self) {
        let tabs: Vec<ModuleTab> = self
            .enabled()
            .iter()
            .map(|m| ModuleTab {
                id: m.id().into(),
                title: m.title().into(),
            })
            .collect();
        for n in self.notches.borrow().iter() {
            n.apply_config(tabs.clone());
        }
        self.update_visibility();
    }

    pub fn toggle(&self) {
        if let Some(n) = self.notches.borrow().first() {
            n.handle(Event::Toggle)
        }
    }

    fn update_visibility(&self) {
        let now: HashSet<String> = self
            .notches
            .borrow()
            .iter()
            .flat_map(|n| n.visible_modules())
            .collect();
        let before = self.shown.replace(now.clone());
        for m in &self.modules {
            match (before.contains(m.id()), now.contains(m.id())) {
                (false, true) => m.on_show(),
                (true, false) => m.on_hide(),
                _ => {}
            }
        }
        self.refresh_peek();
    }

    /// First enabled module with a peek item wins; otherwise show the clock.
    fn refresh_peek(&self) {
        let notches = self.notches.borrow();
        if !notches.iter().any(|n| n.state() == State::Peek) {
            return;
        }
        let text = self
            .enabled()
            .iter()
            .find_map(|m| m.peek())
            .unwrap_or_else(|| jiff::Zoned::now().strftime("%a %-d %b   %H:%M").to_string());
        for n in notches.iter() {
            n.set_peek_text(&text);
        }
    }

    fn alert(&self) {
        for n in self.notches.borrow().iter() {
            n.handle(Event::Alert);
        }
        self.refresh_peek();
    }
}
