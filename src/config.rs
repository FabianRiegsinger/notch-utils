//! `config.toml`: every key optional, missing keys take defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub behavior: Behavior,
    pub display: Display,
    pub appearance: Appearance,
    pub layout: Layout,
    pub modules: Modules,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    pub open_mode: OpenMode,
    pub hotkey_enabled: bool,
    pub hotkey: String,
    pub collapse_delay_ms: u64,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            open_mode: OpenMode::default(),
            hotkey_enabled: false,
            hotkey: "Alt+Space".into(),
            collapse_delay_ms: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    #[default]
    PeekThenClick,
    HoverFull,
    ClickOnly,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    pub target: DisplayTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayTarget {
    #[default]
    Main,
    All,
    FollowMouse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub accent: String,
    pub expanded_width: u32,
    pub expanded_height: u32,
    /// Read at startup only.
    pub renderer: Renderer,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            accent: "#0A84FF".into(),
            expanded_width: 640,
            expanded_height: 300,
            renderer: Renderer::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Black,
    System,
    Custom,
}

/// Software: ~35 MB, a bit more CPU while animating. GPU (OpenGL via femtovg): less CPU
/// while animating, but a ~230 MB driver-init peak and higher steady memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    #[default]
    Software,
    Gpu,
}

impl Renderer {
    /// Slint renderer name.
    pub fn slint_name(self) -> &'static str {
        match self {
            Renderer::Software => "software",
            Renderer::Gpu => "femtovg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub mode: LayoutMode,
    /// Order = display order; a module not listed is disabled.
    pub modules: Vec<String>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            mode: LayoutMode::default(),
            modules: ["timer", "calc", "stats", "clipboard"]
                .map(String::from)
                .to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Tabs,
    Grid,
}

/// Per-module settings: `[modules.<id>]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    pub stats: Stats,
    pub clipboard: Clipboard,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub view: StatsView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsView {
    #[default]
    Graphs,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clipboard {
    pub max_entries: usize,
    /// Save text entries and pins to disk. Images are never saved.
    pub persist: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            max_entries: 50,
            persist: false,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("", "", "notch-utils")
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    /// Missing file → defaults, written to disk so users can find and edit it.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                if let Err(e) = cfg.save(path) {
                    log::warn!("could not write default config to {}: {e}", path.display());
                }
                Ok(cfg)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Write to a temp file then rename, so a crash never leaves a half-written config.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn partial_keeps_other_defaults() {
        let c = Config::parse("[behavior]\nopen_mode = \"click_only\"\n[layout]\nmode = \"grid\"")
            .unwrap();
        assert_eq!(c.behavior.open_mode, OpenMode::ClickOnly);
        assert_eq!(c.behavior.collapse_delay_ms, 300);
        assert_eq!(c.layout.mode, LayoutMode::Grid);
        assert_eq!(c.layout.modules, ["timer", "calc", "stats", "clipboard"]);
        assert_eq!(c.modules.clipboard.max_entries, 50);
        assert_eq!(c.display.target, DisplayTarget::Main);
    }

    #[test]
    fn module_sections() {
        let c = Config::parse(
            "[modules.stats]\nview = \"detailed\"\n[modules.clipboard]\npersist = true",
        )
        .unwrap();
        assert_eq!(c.modules.stats.view, StatsView::Detailed);
        assert!(c.modules.clipboard.persist);
        assert_eq!(c.modules.clipboard.max_entries, 50);
        assert_eq!(c.appearance.renderer, Renderer::Software);
        let g = Config::parse("[appearance]\nrenderer = \"gpu\"").unwrap();
        assert_eq!(g.appearance.renderer.slint_name(), "femtovg");
    }

    #[test]
    fn bad_enum_is_error() {
        assert!(Config::parse("[display]\ntarget = \"nowhere\"").is_err());
    }

    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("notch-utils-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.appearance.theme = Theme::Custom;
        c.layout.modules = vec!["timer".into(), "calc".into()];
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_missing_writes_default() {
        let dir =
            std::env::temp_dir().join(format!("notch-utils-test-missing-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert!(path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
