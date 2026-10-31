//! Owns the notch windows, the modules, settings, hotkey and config reload, and routes
//! between them. Lives on the UI thread; other threads reach it via `App::post`.

use crate::config::{Config, DisplayTarget};
use crate::hotkey::Hotkey;
use crate::ipc::Command;
use crate::modules::{self, Module};
use crate::notch::Notch;
use crate::state::{Event, State};
use crate::{ModuleTab, SettingsWindow, settings};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const FOLLOW_MOUSE_POLL: Duration = Duration::from_millis(400);
const HUD_TIME: Duration = Duration::from_millis(1500);

thread_local! {
    static APP: RefCell<Weak<App>> = RefCell::default();
}

pub struct App {
    me: Weak<App>,
    cfg: Rc<RefCell<Config>>,
    cfg_path: PathBuf,
    /// Last config load / hotkey problem, shown in the settings window.
    error: RefCell<String>,
    modules: Vec<Rc<dyn Module>>,
    notches: RefCell<Vec<Rc<Notch>>>,
    /// Module ids visible in at least one window.
    shown: RefCell<HashSet<String>>,
    settings: RefCell<Option<SettingsWindow>>,
    /// Created once the event loop runs, so OS backends find a live app / message loop.
    hotkey: std::cell::OnceCell<Hotkey>,
    reload_timer: slint::Timer,
    hud_timer: slint::Timer,
    follow_timer: slint::Timer,
}

impl App {
    pub fn new(
        cfg: Config,
        cfg_path: PathBuf,
        error: String,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let app = Rc::new_cyclic(|me: &Weak<App>| {
            let (a, b, c, d) = (me.clone(), me.clone(), me.clone(), me.clone());
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
                hud: Rc::new(move |h| {
                    if let Some(app) = d.upgrade() {
                        app.hud(h)
                    }
                }),
                collapse: Rc::new(move || {
                    if let Some(app) = c.upgrade() {
                        for n in app.notches.borrow().iter() {
                            n.handle(Event::Escape)
                        }
                    }
                }),
            };
            App {
                me: me.clone(),
                cfg: Rc::new(RefCell::new(cfg)),
                cfg_path,
                error: RefCell::new(error),
                modules: modules::all(ctx),
                notches: RefCell::default(),
                shown: RefCell::default(),
                settings: RefCell::default(),
                hotkey: std::cell::OnceCell::new(),
                reload_timer: slint::Timer::default(),
                hud_timer: slint::Timer::default(),
                follow_timer: slint::Timer::default(),
            }
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        app.add_notch(None)?;
        app.apply_all();
        let me = app.me.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(app) = me.upgrade() {
                let _ = app.hotkey.set(Hotkey::new(|| App::post(Command::Toggle)));
                app.apply_all();
            }
        });
        Ok(app)
    }

    /// Run a command on the UI thread; callable from any thread.
    pub fn post(cmd: Command) {
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
                app.command(cmd)
            }
        });
    }

    /// Config file changed on disk; callable from any thread. Debounced.
    pub fn post_reload() {
        let _ = slint::invoke_from_event_loop(|| {
            if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
                let me = app.me.clone();
                app.reload_timer
                    .start(slint::TimerMode::SingleShot, RELOAD_DEBOUNCE, move || {
                        if let Some(app) = me.upgrade() {
                            app.reload()
                        }
                    });
            }
        });
    }

    pub fn command(&self, cmd: Command) {
        match cmd {
            Command::Toggle => self.toggle(),
            Command::Settings => self.open_settings(),
            Command::Shelf(path) => {
                self.drop_file(&path);
                if let Some(n) = self.notches.borrow().first() {
                    n.handle(Event::FileHover);
                    n.select_tab("shelf");
                }
            }
            Command::Usage(json) => {
                for m in &self.modules {
                    m.receive("usage", &json);
                }
            }
            Command::Ping => {}
        }
    }

    fn add_notch(&self, screen: Option<usize>) -> Result<(), slint::PlatformError> {
        let (me, me2, me3) = (self.me.clone(), self.me.clone(), self.me.clone());
        let notch = Notch::new(
            self.cfg.clone(),
            Rc::new(move || {
                if let Some(app) = me.upgrade() {
                    app.update_visibility()
                }
            }),
            Rc::new(move |key| me2.upgrade().is_some_and(|app| app.key(key))),
            Rc::new(move |path| {
                if let Some(app) = me3.upgrade() {
                    app.drop_file(path)
                }
            }),
        )?;
        notch.set_screen(screen);
        for m in &self.modules {
            m.attach(notch.ui());
        }
        self.notches.borrow_mut().push(notch);
        Ok(())
    }

    /// Enabled modules in configured order; unknown ids are skipped.
    fn enabled(&self) -> Vec<Rc<dyn Module>> {
        let cfg = self.cfg.borrow();
        cfg.layout
            .modules
            .iter()
            .filter_map(|id| self.modules.iter().find(|m| m.id() == id).cloned())
            .collect()
    }

    /// Apply the whole config: windows per display, layout, appearance, hotkey.
    fn apply_all(&self) {
        let mut problems = vec![];
        if let Err(e) = self.sync_displays() {
            problems.push(format!("Could not create notch window: {e}"));
        }
        let tabs: Vec<ModuleTab> = self
            .enabled()
            .iter()
            .filter(|m| m.has_view())
            .map(|m| ModuleTab {
                id: m.id().into(),
                title: m.title().into(),
            })
            .collect();
        for id in &self.cfg.borrow().layout.modules {
            if !self.modules.iter().any(|m| m.id() == id) {
                problems.push(format!("Unknown module {id:?} in config"));
            }
        }
        for n in self.notches.borrow().iter() {
            n.apply_config(tabs.clone());
        }
        for m in &self.modules {
            m.config_changed(&self.cfg.borrow());
        }
        let (enabled, spec) = {
            let c = self.cfg.borrow();
            (c.behavior.hotkey_enabled, c.behavior.hotkey.clone())
        };
        if let Some(hk) = self.hotkey.get()
            && let Err(e) = hk.set(enabled, &spec)
        {
            problems.push(e);
        }
        for p in &problems {
            log::warn!("{p}");
        }
        // Keep a config parse error visible until the file parses again.
        let mut err = self.error.borrow_mut();
        if !err.starts_with("config.toml") {
            *err = problems.join("\n");
        }
        drop(err);
        self.update_visibility();
        self.refresh_settings_error();
    }

    /// One notch per target screen. `follow_mouse` moves a single notch, polled cheaply.
    fn sync_displays(&self) -> Result<(), slint::PlatformError> {
        let target = self.cfg.borrow().display.target;
        let count = match self.notches.borrow().first() {
            Some(n) if target == DisplayTarget::All => n.screen_count(),
            _ => 1,
        };
        {
            let mut notches = self.notches.borrow_mut();
            while notches.len() > count {
                if let Some(n) = notches.pop() {
                    n.close()
                }
            }
        }
        while self.notches.borrow().len() < count {
            let i = self.notches.borrow().len();
            self.add_notch(Some(i))?;
        }
        let notches = self.notches.borrow();
        for (i, n) in notches.iter().enumerate() {
            n.set_screen(if target == DisplayTarget::All {
                Some(i)
            } else {
                None
            });
        }
        if target == DisplayTarget::FollowMouse {
            let me = self.me.clone();
            self.follow_timer
                .start(slint::TimerMode::Repeated, FOLLOW_MOUSE_POLL, move || {
                    let Some(app) = me.upgrade() else { return };
                    let notches = app.notches.borrow();
                    let Some(n) = notches.first() else { return };
                    // Don't move a notch away from under the user while it is open.
                    if n.state() == State::Collapsed
                        && let Some(s) = n.cursor_screen()
                    {
                        n.set_screen(Some(s));
                    }
                });
        } else {
            self.follow_timer.stop();
        }
        Ok(())
    }

    fn reload(&self) {
        let text = match std::fs::read_to_string(&self.cfg_path) {
            Ok(t) => t,
            Err(e) => return log::warn!("reload: {e}"),
        };
        match Config::parse(&text) {
            Ok(new) => {
                let was_error = self.error.borrow().starts_with("config.toml");
                if was_error {
                    self.error.borrow_mut().clear();
                }
                if new != *self.cfg.borrow() {
                    log::info!("config reloaded");
                    *self.cfg.borrow_mut() = new;
                    self.apply_all();
                    self.fill_settings();
                } else if was_error {
                    self.refresh_settings_error();
                }
            }
            Err(e) => {
                log::error!("config.toml: {e}; keeping previous settings");
                *self.error.borrow_mut() = format!("config.toml: {e}\nKeeping previous settings.");
                self.refresh_settings_error();
            }
        }
    }

    pub fn toggle(&self) {
        let notches = self.notches.borrow();
        // With several notches, toggle the one on the screen with the mouse.
        let target = notches
            .iter()
            .find(|n| n.screen().is_some() && n.screen() == n.cursor_screen())
            .or_else(|| notches.first());
        if let Some(n) = target {
            n.handle(Event::Toggle)
        }
    }

    /// Give a dropped file to the first enabled module that takes it.
    fn drop_file(&self, path: &std::path::Path) {
        if !self.enabled().iter().any(|m| m.drop_file(path)) {
            log::info!(
                "dropped {} but no module takes files (enable Shelf)",
                path.display()
            );
        }
    }

    /// Offer a key to the visible modules, in tab order.
    fn key(&self, key: &str) -> bool {
        let shown = self.shown.borrow().clone();
        self.enabled()
            .iter()
            .filter(|m| shown.contains(m.id()))
            .any(|m| m.key(key))
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
        let Ok(notches) = self.notches.try_borrow() else {
            return;
        };
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

    /// Flash a HUD for HUD_TIME; repeated calls (e.g. holding a volume key) extend it.
    fn hud(&self, h: crate::modules::Hud) {
        for n in self.notches.borrow().iter() {
            n.set_hud(Some(&h));
            n.handle(Event::Hud);
        }
        let me = self.me.clone();
        self.hud_timer
            .start(slint::TimerMode::SingleShot, HUD_TIME, move || {
                let Some(app) = me.upgrade() else { return };
                for n in app.notches.borrow().iter() {
                    if !n.hovered() {
                        n.handle(Event::HudTimeout);
                    }
                    n.set_hud(None);
                }
                app.refresh_peek();
            });
    }

    fn alert(&self) {
        for n in self.notches.borrow().iter() {
            n.handle(Event::Alert);
        }
        self.refresh_peek();
    }

    // ---- settings window ----

    pub fn open_settings(&self) {
        if self.settings.borrow().is_none() {
            match self.create_settings() {
                Ok(ui) => *self.settings.borrow_mut() = Some(ui),
                Err(e) => return log::error!("settings window: {e}"),
            }
        }
        self.fill_settings();
        if let Some(ui) = &*self.settings.borrow() {
            let _ = ui.show();
            crate::platform::activate_app();
            ui.window().with_winit_window(|w| w.focus_window());
        }
    }

    fn create_settings(&self) -> Result<SettingsWindow, slint::PlatformError> {
        let ui = SettingsWindow::new()?;
        ui.set_config_path(self.cfg_path.display().to_string().into());
        ui.set_renderer_at_start(match self.cfg.borrow().appearance.renderer {
            crate::config::Renderer::Software => 0,
            crate::config::Renderer::Gpu => 1,
        });
        let me = self.me.clone();
        let with = move |f: fn(&App, &SettingsWindow)| {
            let me = me.clone();
            move || {
                if let Some(app) = me.upgrade()
                    && let Some(ui) = &*app.settings.borrow()
                {
                    f(&app, ui)
                }
            }
        };
        ui.on_changed(with(|app, ui| app.settings_changed(ui)));
        ui.on_open_config_file(with(|app, _| settings::open_file(&app.cfg_path)));
        let me = self.me.clone();
        ui.on_module_toggled(move |i, on| {
            let Some(app) = me.upgrade() else { return };
            let Some(ui) = &*app.settings.borrow() else {
                return;
            };
            let model = ui.get_modules();
            if let Some(mut row) = model.row_data(i as usize) {
                row.enabled = on;
                model.set_row_data(i as usize, row);
            }
            app.settings_changed(ui);
        });
        let me = self.me.clone();
        ui.on_module_moved(move |from, to| {
            let Some(app) = me.upgrade() else { return };
            let Some(ui) = &*app.settings.borrow() else {
                return;
            };
            let mut rows: Vec<_> = ui.get_modules().iter().collect();
            if (from as usize) < rows.len() && (to as usize) < rows.len() {
                rows.swap(from as usize, to as usize);
                ui.set_modules(Rc::new(VecModel::from(rows)).into());
            }
            app.settings_changed(ui);
        });
        Ok(ui)
    }

    fn fill_settings(&self) {
        if let Some(ui) = &*self.settings.borrow() {
            settings::to_ui(ui, &self.cfg.borrow(), &self.modules);
            ui.set_error(self.error.borrow().as_str().into());
        }
    }

    fn refresh_settings_error(&self) {
        if let Ok(s) = self.settings.try_borrow()
            && let Some(ui) = &*s
        {
            ui.set_error(self.error.borrow().as_str().into());
        }
    }

    fn settings_changed(&self, ui: &SettingsWindow) {
        let new = settings::from_ui(ui, &self.cfg.borrow());
        if new == *self.cfg.borrow() {
            return;
        }
        // Never overwrite a file the user is mid-way editing: apply in memory only.
        if self.error.borrow().starts_with("config.toml") {
            log::warn!("config.toml has errors; settings change applied but not saved");
        // The watcher sees this save too; reload() then finds nothing changed.
        } else if let Err(e) = new.save(&self.cfg_path) {
            log::error!("saving config: {e}");
        }
        *self.cfg.borrow_mut() = new;
        self.apply_all();
    }
}
