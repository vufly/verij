/// verij-cli/src/config.rs
///
/// User configuration loaded from `~/.config/verij/config.toml` (XDG-aware).
///
/// All fields are optional — missing keys fall back to built-in defaults so an
/// empty (or absent) file is always valid.
///
/// Example config:
/// ```toml
/// [workspace]
/// default_mode = "descend"   # "ask" | "fullscreen" | "descend"
/// prefix = "_vj_"            # override with any string/unicode, e.g. "🔷"
/// sidebar_width = "25%"
///
/// [colors]
/// title        = 6   # Cyan
/// session      = 4   # Blue
/// active_bg    = 1   # Red  (active / attached session)
/// active_fg    = 255 # Bright white
/// selected_bg  = 243 # Medium gray
/// selected_fg  = 0   # Black
/// muted        = 8   # Dark gray
/// error        = 1   # Red
/// spinner      = 6   # Cyan
/// current_mark = 2   # Green
/// ```

use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Workspace default mode
// ---------------------------------------------------------------------------

/// How the workspace pane behaves when switching to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    /// Prompt the user to choose an action (future interactive menu).
    Ask,
    /// Switch and expand the session to full-screen.
    Fullscreen,
    /// Switch and descend into the session's active tab (default).
    #[default]
    Descend,
}

// ---------------------------------------------------------------------------
// Color tokens
// ---------------------------------------------------------------------------

/// ANSI 256-color indices for every UI element.
/// All fields default to the original built-in palette.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    /// Title / header text (default: Cyan = 6).
    pub title: u8,
    /// Session row foreground (default: Blue = 4).
    pub session: u8,
    /// Foreground of the attached (active) session name (default: Red = 1).
    pub attached_fg: u8,
    /// Background of the active-tab badge (default: Red = 1).
    pub active_bg: u8,
    /// Foreground of the active-tab badge (default: Bright white = 255).
    pub active_fg: u8,
    /// Background of the cursor-selected row (default: Bright white = 15).
    pub selected_bg: u8,
    /// Foreground of the cursor-selected row (default: Dark gray = 8).
    pub selected_fg: u8,
    /// Muted / secondary text (default: Dark gray = 8).
    pub muted: u8,
    /// Error text (default: Red = 1).
    pub error: u8,
    /// Connecting spinner (default: Cyan = 6).
    pub spinner: u8,
    /// "Current" marker — fold icon highlight, dialog border (default: Green = 2).
    pub current_mark: u8,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            title: 6,
            session: 4,
            attached_fg: 1,
            active_bg: 1,
            active_fg: 255,
            selected_bg: 15,
            selected_fg: 8,
            muted: 8,
            error: 1,
            spinner: 6,
            current_mark: 2,
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace section
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// How to behave when Enter is pressed on a session row.
    pub default_mode: WorkspaceMode,
    /// Prefix used for Verij host sessions (default: `_vj_`).
    pub prefix: String,
    /// Sidebar pane width passed to the KDL layout (default: `25%`).
    pub sidebar_width: String,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            default_mode: WorkspaceMode::Descend,
            prefix: verij_types::HOST_SESSION_PREFIX.to_string(),
            sidebar_width: "25%".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workspace: WorkspaceConfig,
    pub colors: ColorConfig,
}

impl Config {
    /// Load configuration from `~/.config/verij/config.toml` (XDG-aware).
    ///
    /// Returns `Config::default()` — with built-in defaults — if the file does
    /// not exist or cannot be parsed, logging a warning to stderr in the latter
    /// case.
    pub fn load() -> Self {
        let path = config_path();
        let Some(path) = path else {
            return Self::default();
        };

        let Ok(text) = std::fs::read_to_string(&path) else {
            // File absent → silent default
            return Self::default();
        };

        match toml::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "[verij] Warning: failed to parse config at {}: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Effective host session prefix: config value if set, otherwise the compiled-in constant.
    pub fn prefix(&self) -> &str {
        &self.workspace.prefix
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Resolves the config file path: `$XDG_CONFIG_HOME/verij/config.toml`
/// or `~/.config/verij/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".config")
    };
    Some(base.join("verij").join("config.toml"))
}

/// Write a starter config file to `~/.config/verij/config.toml` if it does
/// not already exist.  Used by `verij config init`.
pub fn write_default_config() -> anyhow::Result<()> {
    let path = config_path()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine config path (HOME not set?)"))?;

    if path.exists() {
        println!("Config already exists at {}", path.display());
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, DEFAULT_CONFIG_TOML)?;
    println!("Created default config at {}", path.display());
    Ok(())
}

/// Canonical default config text, written by `verij config init`.
const DEFAULT_CONFIG_TOML: &str = r#"# Verij configuration — ~/.config/verij/config.toml
# All keys are optional; uncomment and edit as needed.

[workspace]
# How to behave when pressing Enter on a session row.
# Options: "descend" (default) | "fullscreen" | "ask"
default_mode = "descend"

# Host-session prefix — sessions starting with this string are hidden from the
# workspace list.  Change to any string or unicode symbol, e.g. "🔷".
prefix = "_vj_"

# Sidebar pane width as a percentage or fixed cell count, e.g. "25%" or "30".
sidebar_width = "25%"

[colors]
# ANSI 256-color indices for every UI element.
title        = 6    # Cyan
session      = 4    # Blue
attached_fg  = 1    # Red   — attached session name foreground
active_bg    = 1    # Red   — active-tab badge background
active_fg    = 255  # Bright white — active-tab badge foreground
selected_bg  = 15   # Bright white — cursor row background
selected_fg  = 8    # Dark gray — cursor row foreground
muted        = 8    # Dark gray — secondary text
error        = 1    # Red — error message
spinner      = 6    # Cyan — connecting spinner
current_mark = 2    # Green — fold icon / dialog border
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.workspace.prefix, "_vj_");
        assert_eq!(cfg.workspace.sidebar_width, "25%");
        assert!(matches!(cfg.workspace.default_mode, WorkspaceMode::Descend));
        assert_eq!(cfg.colors.active_bg, 1);
        assert_eq!(cfg.colors.active_fg, 255);
        assert_eq!(cfg.colors.selected_bg, 15);
        assert_eq!(cfg.colors.selected_fg, 8);
    }

    #[test]
    fn test_partial_toml() {
        let toml = r#"
[workspace]
prefix = "🔷"
sidebar_width = "30%"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.workspace.prefix, "🔷");
        assert_eq!(cfg.workspace.sidebar_width, "30%");
        // defaults preserved for unset keys
        assert_eq!(cfg.colors.active_bg, 1);
    }

    #[test]
    fn test_full_toml() {
        let toml = r#"
[workspace]
default_mode = "fullscreen"
prefix = "🔷"
sidebar_width = "30%"

[colors]
title        = 5
session      = 3
active_bg    = 9
active_fg    = 0
selected_bg  = 240
selected_fg  = 15
muted        = 7
error        = 9
spinner      = 5
current_mark = 10
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(matches!(cfg.workspace.default_mode, WorkspaceMode::Fullscreen));
        assert_eq!(cfg.colors.title, 5);
        assert_eq!(cfg.colors.active_bg, 9);
    }
}
