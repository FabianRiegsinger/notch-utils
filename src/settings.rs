//! Maps `Config` to and from the settings window. Enum ↔ ComboBox index tables live here.

use crate::config::{Config, DisplayTarget, LayoutMode, OpenMode, Renderer, StatsView, Theme};
use crate::modules::Module;
use crate::{ModuleEntry, ModuleSet, SettingsWindow};
use slint::{Model, VecModel};
use std::rc::Rc;

const OPEN: [OpenMode; 3] = [
    OpenMode::PeekThenClick,
    OpenMode::HoverFull,
    OpenMode::ClickOnly,
];
const DISPLAY: [DisplayTarget; 3] = [
    DisplayTarget::Main,
    DisplayTarget::All,
    DisplayTarget::FollowMouse,
];
const THEME: [Theme; 3] = [Theme::Black, Theme::System, Theme::Custom];
const LAYOUT: [LayoutMode; 2] = [LayoutMode::Tabs, LayoutMode::Grid];
const RENDERER: [Renderer; 2] = [Renderer::Software, Renderer::Gpu];
const STATS_VIEW: [StatsView; 2] = [StatsView::Graphs, StatsView::Detailed];

/// One-click module presets: (name, description, module ids in tab order).
pub const SETS: [(&str, &str, &[&str]); 4] = [
    (
        "Everyday",
        "Music, timer, calendar, shelf, clipboard, Keep Awake and hints",
        &[
            "home",
            "music",
            "timer",
            "calendar",
            "shelf",
            "clipboard",
            "awake",
            "hud",
            "hints",
        ],
    ),
    (
        "Developer",
        "Terminal, system stats, AI usage, clipboard, calculator and Keep Awake",
        &[
            "home",
            "terminal",
            "stats",
            "ai",
            "clipboard",
            "calc",
            "awake",
            "hud",
            "hints",
        ],
    ),
    (
        "Minimal",
        "Just home, a timer and the hints",
        &["home", "timer", "hud", "hints"],
    ),
    (
        "Everything",
        "Every module",
        &[
            "home",
            "music",
            "timer",
            "calc",
            "stats",
            "clipboard",
            "shelf",
            "terminal",
            "calendar",
            "ai",
            "awake",
            "hud",
            "hints",
        ],
    ),
];

/// The set whose module list matches `modules` exactly.
pub fn active_set(modules: &[String]) -> i32 {
    SETS.iter()
        .position(|(_, _, ids)| {
            ids.len() == modules.len() && ids.iter().zip(modules).all(|(a, b)| a == b)
        })
        .map_or(-1, |i| i as i32)
}

/// Module rows with set `i` enabled in its order, every other module disabled after them.
pub fn entries_for_set(i: usize, all: &[Rc<dyn Module>]) -> Vec<ModuleEntry> {
    let ids = SETS[i.min(SETS.len() - 1)].2;
    let mut cfg = Config::default();
    cfg.layout.modules = ids.iter().map(|s| s.to_string()).collect();
    module_entries(&cfg, all)
}

fn index<T: PartialEq>(table: &[T], v: &T) -> i32 {
    table.iter().position(|x| x == v).unwrap_or(0) as i32
}

fn pick<T: Copy>(table: &[T], i: i32) -> T {
    table[(i.max(0) as usize).min(table.len() - 1)]
}

/// Enabled modules in config order first, then the disabled ones.
pub fn module_entries(cfg: &Config, all: &[Rc<dyn Module>]) -> Vec<ModuleEntry> {
    let entry = |m: &Rc<dyn Module>, enabled| ModuleEntry {
        id: m.id().into(),
        title: m.title().into(),
        enabled,
    };
    let mut out: Vec<ModuleEntry> = cfg
        .layout
        .modules
        .iter()
        .filter_map(|id| all.iter().find(|m| m.id() == id))
        .map(|m| entry(m, true))
        .collect();
    for m in all {
        if !cfg.layout.modules.iter().any(|id| id == m.id()) {
            out.push(entry(m, false));
        }
    }
    out
}

pub fn to_ui(ui: &SettingsWindow, cfg: &Config, all: &[Rc<dyn Module>]) {
    ui.set_open_mode(index(&OPEN, &cfg.behavior.open_mode));
    ui.set_hotkey_enabled(cfg.behavior.hotkey_enabled);
    ui.set_start_at_login(cfg.behavior.start_at_login);
    ui.set_hotkey(cfg.behavior.hotkey.as_str().into());
    ui.set_collapse_delay(cfg.behavior.collapse_delay_ms.min(5000) as i32);
    ui.set_display_target(index(&DISPLAY, &cfg.display.target));
    ui.set_theme(index(&THEME, &cfg.appearance.theme));
    ui.set_accent(cfg.appearance.accent.as_str().into());
    ui.set_expanded_width(cfg.appearance.expanded_width as i32);
    ui.set_expanded_height(cfg.appearance.expanded_height as i32);
    ui.set_layout_mode(index(&LAYOUT, &cfg.layout.mode));
    ui.set_renderer(index(&RENDERER, &cfg.appearance.renderer));
    ui.set_modules(Rc::new(VecModel::from(module_entries(cfg, all))).into());
    let sets: Vec<ModuleSet> = SETS
        .iter()
        .map(|(n, d, _)| ModuleSet {
            name: (*n).into(),
            description: (*d).into(),
        })
        .collect();
    ui.set_sets(Rc::new(VecModel::from(sets)).into());
    ui.set_active_set(active_set(&cfg.layout.modules));
    ui.set_calendar_urls(cfg.modules.calendar.urls.join("\n").into());
    ui.set_stats_view(index(&STATS_VIEW, &cfg.modules.stats.view));
    ui.set_clip_max(cfg.modules.clipboard.max_entries.min(500) as i32);
    ui.set_clip_persist(cfg.modules.clipboard.persist);
    ui.set_hud_volume(cfg.modules.hud.volume);
    ui.set_hud_battery(cfg.modules.hud.battery);
    ui.set_hint_caps(cfg.modules.hints.caps_lock);
    ui.set_hint_mic_camera(cfg.modules.hints.mic_camera);
    ui.set_hint_connection(cfg.modules.hints.connection);
    ui.set_hint_focus(cfg.modules.hints.focus);
}

/// Start from `base` so keys the window doesn't show are kept.
pub fn from_ui(ui: &SettingsWindow, base: &Config) -> Config {
    let mut c = base.clone();
    c.behavior.open_mode = pick(&OPEN, ui.get_open_mode());
    c.behavior.hotkey_enabled = ui.get_hotkey_enabled();
    c.behavior.start_at_login = ui.get_start_at_login();
    c.behavior.hotkey = ui.get_hotkey().trim().to_string();
    c.behavior.collapse_delay_ms = ui.get_collapse_delay().max(0) as u64;
    c.display.target = pick(&DISPLAY, ui.get_display_target());
    c.appearance.theme = pick(&THEME, ui.get_theme());
    c.appearance.accent = ui.get_accent().trim().to_string();
    c.appearance.expanded_width = ui.get_expanded_width().max(0) as u32;
    c.appearance.expanded_height = ui.get_expanded_height().max(0) as u32;
    c.layout.mode = pick(&LAYOUT, ui.get_layout_mode());
    c.appearance.renderer = pick(&RENDERER, ui.get_renderer());
    c.layout.modules = ui
        .get_modules()
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.id.to_string())
        .collect();
    c.modules.stats.view = pick(&STATS_VIEW, ui.get_stats_view());
    c.modules.clipboard.max_entries = ui.get_clip_max().max(1) as usize;
    c.modules.clipboard.persist = ui.get_clip_persist();
    c.modules.calendar.urls = ui
        .get_calendar_urls()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    c.modules.hud.volume = ui.get_hud_volume();
    c.modules.hud.battery = ui.get_hud_battery();
    c.modules.hints.caps_lock = ui.get_hint_caps();
    c.modules.hints.mic_camera = ui.get_hint_mic_camera();
    c.modules.hints.connection = ui.get_hint_connection();
    c.modules.hints.focus = ui.get_hint_focus();
    c
}

pub fn open_file(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("notepad").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = r {
        log::warn!("could not open {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_are_recognised() {
        let everyday: Vec<String> = SETS[0].2.iter().map(|s| s.to_string()).collect();
        assert_eq!(active_set(&everyday), 0);
        assert_eq!(active_set(&["timer".to_string()]), -1);
        for (_, _, ids) in SETS {
            assert!(
                ids.iter().all(|id| SETS[3].2.contains(id)),
                "Everything covers every set"
            );
        }
    }

    #[test]
    fn index_tables_round_trip() {
        for (i, v) in OPEN.iter().enumerate() {
            assert_eq!(pick(&OPEN, index(&OPEN, v)), *v);
            assert_eq!(index(&OPEN, v), i as i32);
        }
        assert_eq!(pick(&THEME, 99), Theme::Custom);
        assert_eq!(pick(&THEME, -1), Theme::Black);
    }
}
