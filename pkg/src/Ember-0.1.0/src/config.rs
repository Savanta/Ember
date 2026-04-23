use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

use crate::toast::animation::AnimStyle;

// ── Config structs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub dnd: DndConfig,
    #[serde(default)]
    pub toast: ToastConfig,
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    /// Per-application overrides: `[[app]]` array in TOML.
    #[serde(default, rename = "app")]
    pub apps: Vec<AppRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub socket_path: String,
    pub history_db:  String,
    pub max_history: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: String::new(),
            history_db:  String::new(),
            max_history: 500,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DndConfig {
    pub enabled:  bool,
    pub schedule: Option<DndSchedule>,
}

/// Automatic DND time window.  `from` and `to` are local hours (0-23, 24h).
/// An overnight range (e.g. from=22, to=8) is supported.
#[derive(Debug, Clone, Deserialize)]
pub struct DndSchedule {
    pub from: u8,
    pub to:   u8,
}

/// Per-application notification rules (`[[app]]` in config.toml).
#[derive(Debug, Clone, Deserialize)]
pub struct AppRule {
    /// App name to match (case-insensitive substring match).
    pub name: String,
    /// Override timeout in milliseconds (0 = never expire). `None` = use global default.
    pub timeout_ms: Option<u64>,
    /// Override urgency: "low", "normal", "critical". `None` = keep original.
    pub urgency: Option<String>,
    /// If `true`, suppress toast entirely (still stored in history).
    #[serde(default)]
    pub mute: bool,
}

impl AppRule {
    /// Returns true if this rule matches the given app name.
    pub fn matches(&self, app_name: &str) -> bool {
        app_name.to_lowercase().contains(&self.name.to_lowercase())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub focus_next:      String,
    pub focus_prev:      String,
    pub dismiss_focused: String,
    pub invoke_default:  String,
    pub action_1:        String,
    pub action_2:        String,
    pub action_3:        String,
    pub open_center:     String,
    pub toggle_dnd:      String,
    pub clear_all:       String,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            focus_next:      "ctrl+grave".into(),
            focus_prev:      "ctrl+shift+grave".into(),
            dismiss_focused: "ctrl+Delete".into(),
            invoke_default:  "ctrl+Return".into(),
            action_1:        "ctrl+1".into(),
            action_2:        "ctrl+2".into(),
            action_3:        "ctrl+3".into(),
            open_center:     "super+n".into(),
            toggle_dnd:      "super+shift+n".into(),
            clear_all:       "ctrl+shift+Delete".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ToastConfig {
    pub position:         String,
    pub animation:        AnimStyle,
    pub width:            u32,
    pub max_visible:      u32,
    pub gap:              i32,
    pub margin_x:         i32,
    pub margin_y:         i32,
    pub timeout_normal:   u64,
    pub timeout_critical: u64,
    pub timeout_low:      u64,
    pub theme:            ThemeConfig,
}

impl Default for ToastConfig {
    fn default() -> Self {
        Self {
            position:         "top-right".into(),
            animation:        AnimStyle::default(),
            width:            440,
            max_visible:      5,
            gap:              10,
            margin_x:         18,
            margin_y:         50,
            timeout_normal:   5000,
            timeout_critical: 0,
            timeout_low:      3000,
            theme:            ThemeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub bg_normal:        String,
    pub bg_low:           String,
    pub bg_critical:      String,
    pub fg:               String,
    pub fg_dim:           String,
    pub font:             String,
    pub border_radius:    f64,
    pub padding:          f64,
    pub border_width:     f64,
    pub border_normal:    String,
    pub border_critical:  String,
    pub accent_normal:    String,
    pub accent_low:       String,
    pub accent_critical:  String,
    pub icon_size:        u32,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            bg_normal:        "#282828".into(),
            bg_low:           "#1d2021".into(),
            bg_critical:      "#9d0006".into(),
            fg:               "#ebdbb2".into(),
            fg_dim:           "#bdae93".into(),
            font:             "Noto Sans 11".into(),
            border_radius:    8.0,
            padding:          16.0,
            border_width:     1.0,
            border_normal:    "#665c54".into(),
            border_critical:  "#fb4934".into(),
            accent_normal:    "#458588".into(),
            accent_low:       "#458588".into(),
            accent_critical:  "#fb4934".into(),
            icon_size:        32,
        }
    }
}

// ── Loading ────────────────────────────────────────────────────────────────────

impl Config {
    /// Load config: start from built-in defaults, then overlay the user file
    /// from `$XDG_CONFIG_HOME/ember/config.toml` if it exists.
    pub fn load() -> Result<Self> {
        let defaults: Config =
            toml::from_str(include_str!("../config/default.toml"))
                .context("parse embedded default config")?;

        let user_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ember/config.toml");

        if user_path.exists() {
            let src = std::fs::read_to_string(&user_path)
                .with_context(|| format!("read {}", user_path.display()))?;
            let user: Config =
                toml::from_str(&src)
                    .with_context(|| format!("parse {}", user_path.display()))?;
            return Ok(user);
        }

        Ok(defaults)
    }

    /// Load config from an explicit path supplied via --config.
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        let src = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&src)
            .with_context(|| format!("parse {}", path.display()))
    }

    /// Resolve the Unix socket path (use XDG_RUNTIME_DIR if not set in config).
    pub fn socket_path(&self) -> PathBuf {
        resolve_path_or(
            &self.daemon.socket_path,
            dirs::runtime_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("ember.sock"),
        )
    }

    /// Resolve the SQLite history database path.
    pub fn db_path(&self) -> PathBuf {
        resolve_path_or(
            &self.daemon.history_db,
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ember/history.db"),
        )
    }
}

fn resolve_path_or(s: &str, default: PathBuf) -> PathBuf {
    if s.is_empty() {
        return default;
    }
    if s.starts_with("~/")
        && let Some(home) = dirs::home_dir() {
            return home.join(&s[2..]);
        }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AppRule::matches ──────────────────────────────────────────────────────

    fn rule(name: &str) -> AppRule {
        AppRule { name: name.into(), timeout_ms: None, urgency: None, mute: false }
    }

    #[test]
    fn app_rule_matches_exact_name() {
        assert!(rule("firefox").matches("firefox"));
    }

    #[test]
    fn app_rule_matches_case_insensitive() {
        assert!(rule("Firefox").matches("firefox"));
        assert!(rule("firefox").matches("Firefox"));
    }

    #[test]
    fn app_rule_matches_substring() {
        assert!(rule("fire").matches("Mozilla Firefox"));
    }

    #[test]
    fn app_rule_no_match_different_name() {
        assert!(!rule("chrome").matches("firefox"));
    }

    #[test]
    fn app_rule_empty_name_matches_everything() {
        // Empty substring is contained in every string
        assert!(rule("").matches("anything"));
        assert!(rule("").matches(""));
    }

    // ── Config toml parsing ───────────────────────────────────────────────────

    #[test]
    fn config_parses_empty_toml_with_defaults() {
        let cfg: Config = toml::from_str("").expect("empty toml should use defaults");
        assert!(!cfg.dnd.enabled);
        assert_eq!(cfg.toast.position, "top-right");
        assert_eq!(cfg.toast.width, 440);
        assert_eq!(cfg.daemon.max_history, 500);
    }

    #[test]
    fn config_parses_dnd_enabled() {
        let cfg: Config = toml::from_str("[dnd]\nenabled = true").expect("parse");
        assert!(cfg.dnd.enabled);
    }

    #[test]
    fn config_parses_toast_position() {
        let cfg: Config = toml::from_str("[toast]\nposition = \"bottom-left\"").expect("parse");
        assert_eq!(cfg.toast.position, "bottom-left");
    }

    #[test]
    fn config_parses_app_rules() {
        let toml = "[[app]]\nname = \"slack\"\nmute = true\n";
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.apps.len(), 1);
        assert_eq!(cfg.apps[0].name, "slack");
        assert!(cfg.apps[0].mute);
    }

    #[test]
    fn config_parses_multiple_app_rules() {
        let toml = "[[app]]\nname = \"spotify\"\n[[app]]\nname = \"discord\"\nmute = true\n";
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.apps.len(), 2);
    }

    // ── socket_path / db_path fallbacks ───────────────────────────────────────

    #[test]
    fn socket_path_uses_explicit_value_when_set() {
        let cfg: Config = toml::from_str("[daemon]\nsocket_path = \"/tmp/my.sock\"").expect("parse");
        assert_eq!(cfg.socket_path(), PathBuf::from("/tmp/my.sock"));
    }

    #[test]
    fn db_path_uses_explicit_value_when_set() {
        let cfg: Config = toml::from_str("[daemon]\nhistory_db = \"/tmp/ember.db\"").expect("parse");
        assert_eq!(cfg.db_path(), PathBuf::from("/tmp/ember.db"));
    }

    #[test]
    fn socket_path_falls_back_to_xdg_runtime() {
        let cfg: Config = toml::from_str("").expect("parse");
        let path = cfg.socket_path();
        // Must end in ember.sock regardless of XDG_RUNTIME_DIR value
        assert_eq!(path.file_name().unwrap(), "ember.sock");
    }

    #[test]
    fn db_path_falls_back_to_xdg_data() {
        let cfg: Config = toml::from_str("").expect("parse");
        let path = cfg.db_path();
        assert_eq!(path.file_name().unwrap(), "history.db");
    }

    #[test]
    fn socket_path_expands_tilde() {
        let cfg: Config = toml::from_str("[daemon]\nsocket_path = \"~/run/ember.sock\"").expect("parse");
        let path = cfg.socket_path();
        // Must not start with ~ after expansion
        assert!(!path.starts_with("~"));
        assert_eq!(path.file_name().unwrap(), "ember.sock");
    }
}
