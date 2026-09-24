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
/// sidebar_width = "25%"
///
/// [tui]
/// single_click_action = true
///
/// [zellij]
/// pane_frame_style = "titles"
/// focus_follows_mouse = true
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

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// A scalar value accepted by Zellij's `options` command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ZellijOption {
    Boolean(bool),
    Integer(i64),
    String(String),
}

impl std::fmt::Display for ZellijOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Boolean(value) => value.fmt(f),
            Self::Integer(value) => value.fmt(f),
            Self::String(value) => f.write_str(value),
        }
    }
}

/// Verij TUI behavior options.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Attach or enter selected tree item from a single mouse click (default: true).
    #[serde(default = "default_true")]
    pub single_click_action: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            single_click_action: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Zellij options applied when creating a Verij host session.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ZellijConfig {
    #[serde(flatten)]
    pub options: BTreeMap<String, ZellijOption>,
}

impl ZellijConfig {
    /// Merge Verij host defaults with user-supplied Zellij options.
    pub fn host_options(&self) -> BTreeMap<String, ZellijOption> {
        let mut options = BTreeMap::from([
            (
                "focus_follows_mouse".to_string(),
                ZellijOption::Boolean(true),
            ),
            (
                "pane_frame_style".to_string(),
                ZellijOption::String("titles".to_string()),
            ),
        ]);
        options.extend(self.options.clone());
        options
    }
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
    /// Sidebar pane width passed to the KDL layout (default: `25%`).
    pub sidebar_width: String,
    /// Format string for the Workspace pane name when a session is active.
    /// `{session}`, `{tab}`/`{active_tab}`, and `{pane}`/`{active_pane}` are
    /// replaced with inner active names. Use `{if variable}...{else}...{endif}`
    /// for optional sections.
    #[serde(alias = "tab_format")]
    pub pane_format: String,
    /// Workspace pane name when no inner session is attached yet (default: `"Workspace"`).
    #[serde(alias = "tab_default")]
    pub pane_default: String,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            default_mode: WorkspaceMode::Descend,
            sidebar_width: "25%".to_string(),
            pane_format: "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}"
                .to_string(),
            pane_default: "Workspace".to_string(),
        }
    }
}

impl WorkspaceConfig {
    /// Render the Workspace pane name using inner session, tab, and pane names.
    pub fn format_pane_name(
        &self,
        session: &str,
        tab: Option<&str>,
        pane: Option<&str>,
    ) -> String {
        let values = [
            ("session", Some(session)),
            ("tab", tab),
            ("active_tab", tab),
            ("pane", pane),
            ("active_pane", pane),
        ];
        let rendered = render_conditionals(&self.pane_format, &values);

        rendered
            .replace("{session}", session)
            .replace("{tab}", tab.unwrap_or(""))
            .replace("{active_tab}", tab.unwrap_or(""))
            .replace("{pane}", pane.unwrap_or(""))
            .replace("{active_pane}", pane.unwrap_or(""))
    }
}

fn render_conditionals(template: &str, values: &[(&str, Option<&str>)]) -> String {
    let mut output = String::new();
    let mut remaining = template;

    while let Some(start) = remaining.find("{if ") {
        output.push_str(&remaining[..start]);
        let condition_start = start + "{if ".len();
        let Some(condition_end) = remaining[condition_start..].find('}') else {
            output.push_str(&remaining[start..]);
            return output;
        };
        let condition_end = condition_start + condition_end;
        let condition = remaining[condition_start..condition_end].trim();
        let body_start = condition_end + 1;
        let Some(end_offset) = remaining[body_start..].find("{endif}") else {
            output.push_str(&remaining[start..]);
            return output;
        };
        let body_end = body_start + end_offset;
        let body = &remaining[body_start..body_end];
        let (when_true, when_false) = body.split_once("{else}").unwrap_or((body, ""));
        let value = values
            .iter()
            .find(|(name, _)| *name == condition)
            .and_then(|(_, value)| *value);
        output.push_str(if value.is_some_and(|value| !value.is_empty()) {
            when_true
        } else {
            when_false
        });
        remaining = &remaining[body_end + "{endif}".len()..];
    }

    output.push_str(remaining);
    output
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workspace: WorkspaceConfig,
    pub tui: TuiConfig,
    pub zellij: ZellijConfig,
    pub colors: ColorConfig,
}

/// Durable attachment state keyed by canonical host-session name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HostsConfig {
    pub hosts: BTreeMap<String, HostAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostAttachment {
    pub last_session: String,
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

/// Resolves durable per-host attachment state beside the regular config file.
pub fn hosts_path() -> Option<PathBuf> {
    config_path().map(|path| path.with_file_name("hosts.toml"))
}

/// Loads durable host attachment state. Missing or invalid files fall back to empty state.
pub fn load_hosts() -> HostsConfig {
    let Some(path) = hosts_path() else {
        return HostsConfig::default();
    };

    let Ok(text) = std::fs::read_to_string(&path) else {
        return HostsConfig::default();
    };

    match toml::from_str::<HostsConfig>(&text) {
        Ok(hosts) => hosts,
        Err(error) => {
            eprintln!(
                "[verij] Warning: failed to parse host state at {}: {error}",
                path.display()
            );
            HostsConfig::default()
        }
    }
}

/// Returns the last inner session attached to a host, if one was recorded.
pub fn last_host_session(host: &str) -> Option<String> {
    load_hosts()
        .hosts
        .get(host)
        .map(|attachment| attachment.last_session.clone())
}

/// Records the last inner session attached to a host using an atomic file replace.
pub fn set_last_host_session(host: &str, session: &str) -> anyhow::Result<()> {
    let Some(path) = hosts_path() else {
        anyhow::bail!("Cannot determine host state path (HOME not set?)");
    };

    let mut hosts = load_hosts();
    hosts.hosts.insert(
        host.to_string(),
        HostAttachment {
            last_session: session.to_string(),
        },
    );
    write_hosts(&path, &hosts)
}

pub fn rename_host_attachment(old: &str, new: &str) -> anyhow::Result<()> {
    let Some(path) = hosts_path() else {
        anyhow::bail!("Cannot determine host state path");
    };
    let mut hosts = load_hosts();
    if hosts.hosts.contains_key(new) {
        anyhow::bail!("Attachment state for '{new}' already exists");
    }
    if let Some(record) = hosts.hosts.remove(old) {
        hosts.hosts.insert(new.to_owned(), record);
        write_hosts(&path, &hosts)?;
    }
    Ok(())
}


fn write_hosts(path: &std::path::Path, hosts: &HostsConfig) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Host state path has no parent"))?;
    std::fs::create_dir_all(parent)?;

    let content = toml::to_string_pretty(hosts)?;
    let temporary = path.with_extension(format!("toml.tmp-{}", std::process::id()));
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, path).map_err(|error| {
        anyhow::anyhow!(
            "Failed to replace host state {} with {}: {error}",
            path.display(),
            temporary.display()
        )
    })
}

/// Write a starter config file to `~/.config/verij/config.toml` if it does
/// not already exist.  Used by `verij config init`.
pub fn write_default_config() -> anyhow::Result<()> {
    crate::layout::init_user_layout()?;
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

# Sidebar pane width as a percentage or fixed cell count, e.g. "25%" or "30".
sidebar_width = "25%"

# Workspace pane name format when an inner session is active.
# {session}, {tab}/{active_tab}, and {pane}/{active_pane} are replaced with inner active names.
# Use {if variable}...{else}...{endif} for optional sections.
# Example: "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}"
pane_format = "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}"

# Workspace pane name when no inner session is attached yet.
pane_default = "Workspace"

[tui]
# Attach or enter a tree item with one click instead of a double-click.
single_click_action = true

[zellij]
# Zellij options applied when creating a Verij host.
pane_frame_style = "titles"
focus_follows_mouse = true

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
        assert_eq!(cfg.workspace.sidebar_width, "25%");
        assert!(matches!(cfg.workspace.default_mode, WorkspaceMode::Descend));
        assert_eq!(cfg.colors.active_bg, 1);
        assert_eq!(cfg.colors.active_fg, 255);
        assert_eq!(cfg.colors.selected_bg, 15);
        assert_eq!(cfg.colors.selected_fg, 8);
        assert!(cfg.tui.single_click_action);
        assert_eq!(
            cfg.zellij.host_options()["pane_frame_style"],
            ZellijOption::String("titles".to_string())
        );
        assert_eq!(
            cfg.zellij.host_options()["focus_follows_mouse"],
            ZellijOption::Boolean(true)
        );
    }

    #[test]
    fn test_partial_toml() {
        let toml = r#"
[workspace]
sidebar_width = "30%"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.workspace.sidebar_width, "30%");
        // defaults preserved for unset keys
        assert_eq!(cfg.colors.active_bg, 1);

        let cfg: Config = toml::from_str("[tui]\n").unwrap();
        assert!(cfg.tui.single_click_action);
    }

    #[test]
    fn test_zellij_options_override() {
        let cfg: Config = toml::from_str(
            "[tui]\nsingle_click_action = false\n\n[zellij]\npane_frame_style = 'none'\nfocus_follows_mouse = false\nscroll_buffer_size = 5000\n",
        )
        .unwrap();
        let options = cfg.zellij.host_options();
        assert!(!cfg.tui.single_click_action);
        assert_eq!(
            options["pane_frame_style"],
            ZellijOption::String("none".to_string())
        );
        assert_eq!(options["focus_follows_mouse"], ZellijOption::Boolean(false));
        assert_eq!(options["scroll_buffer_size"], ZellijOption::Integer(5000));
        assert!(toml::from_str::<Config>("[zellij]\nmouse_mode = []\n").is_err());
    }


    #[test]
    fn test_pane_format_variables() {
        let config = WorkspaceConfig {
            pane_format: "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}".to_string(),
            ..WorkspaceConfig::default()
        };

        assert_eq!(
            config.format_pane_name("backend", Some("editor"), Some("shell")),
            "backend | editor | shell"
        );
        assert_eq!(config.format_pane_name("backend", None, None), "backend");
        assert_eq!(
            config.format_pane_name("backend", Some("editor"), None),
            "backend | editor"
        );
        assert_eq!(
            config.format_pane_name("backend", None, Some("shell")),
            "backend | shell"
        );

        let fallback = WorkspaceConfig {
            pane_format: "{if pane}{pane}{else}no pane{endif}".to_string(),
            ..WorkspaceConfig::default()
        };
        assert_eq!(
            fallback.format_pane_name("backend", None, Some("shell")),
            "shell"
        );
        assert_eq!(fallback.format_pane_name("backend", None, None), "no pane");
    }

    #[test]
    fn test_full_toml() {
        let toml = r#"
[workspace]
default_mode = "fullscreen"
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

    #[test]
    fn test_hosts_toml_is_keyed_by_host() {
        let hosts: HostsConfig = toml::from_str(
            r#"
[hosts."_vj_project"]
last_session = "backend"

[hosts."_vj_personal"]
last_session = "shell"
"#,
        )
        .unwrap();

        assert_eq!(
            hosts.hosts.get("_vj_project").unwrap().last_session,
            "backend"
        );
        assert_eq!(
            hosts.hosts.get("_vj_personal").unwrap().last_session,
            "shell"
        );
    }
}
