use anyhow::{bail, Context, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_LAYOUT: &str = include_str!("../../layouts/verij.kdl");

// ---------------------------------------------------------------------------
// Layout configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub verij_bin: String,
    pub sidebar_size: String,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            verij_bin: "verij".to_string(),
            sidebar_size: "25%".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolves the absolute path to `verij_plugin.wasm`.
///
/// Search order:
/// 1. Explicit CLI argument (`custom_path`)
/// 2. `VERIJ_PLUGIN_PATH` environment variable
/// 3. Relative to current executable (`target/`, sibling, or `dist/`)
/// 4. Relative to current working directory (`dist/` or `target/wasm32-wasip1/...`)
/// 5. Standard system/user share directories (`~/.local/share/verij/...`, `/usr/local/share/...`)
pub fn resolve_plugin_path(custom_path: Option<&Path>) -> Result<PathBuf> {
    // 1. Explicit CLI argument
    if let Some(path) = custom_path {
        if path.exists() {
            return path.canonicalize().with_context(|| {
                format!("Failed to canonicalize plugin path: {}", path.display())
            });
        }
        bail!("Specified plugin path does not exist: {}", path.display());
    }

    // 2. Environment variable VERIJ_PLUGIN_PATH
    if let Ok(env_path) = std::env::var("VERIJ_PLUGIN_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return p.canonicalize().with_context(|| {
                format!("Failed to canonicalize VERIJ_PLUGIN_PATH: {}", p.display())
            });
        }
        bail!("VERIJ_PLUGIN_PATH does not exist: {}", p.display());
    }

    // 3. Search relative to current executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidates = [
                exe_dir.join("verij_plugin.wasm"),
                exe_dir.join("dist").join("verij_plugin.wasm"),
                exe_dir.join("../../dist/verij_plugin.wasm"),
                exe_dir.join("../../target/wasm32-wasip1/release/verij_plugin.wasm"),
                exe_dir.join("../../target/wasm32-wasip1/debug/verij_plugin.wasm"),
            ];
            for cand in &candidates {
                if cand.exists() {
                    if let Ok(canon) = cand.canonicalize() {
                        return Ok(canon);
                    }
                }
            }
        }
    }

    // 4. Relative to current working directory
    let cwd_candidates = [
        PathBuf::from("dist/verij_plugin.wasm"),
        PathBuf::from("target/wasm32-wasip1/release/verij_plugin.wasm"),
        PathBuf::from("target/wasm32-wasip1/debug/verij_plugin.wasm"),
    ];
    for cand in &cwd_candidates {
        if cand.exists() {
            if let Ok(canon) = cand.canonicalize() {
                return Ok(canon);
            }
        }
    }

    // 5. Standard user data / system paths
    if let Ok(home) = std::env::var("HOME") {
        let user_share = PathBuf::from(home).join(".local/share/verij/verij_plugin.wasm");
        if user_share.exists() {
            if let Ok(canon) = user_share.canonicalize() {
                return Ok(canon);
            }
        }
    }

    let sys_candidates = [
        PathBuf::from("/usr/local/share/verij/verij_plugin.wasm"),
        PathBuf::from("/usr/share/verij/verij_plugin.wasm"),
    ];
    for cand in &sys_candidates {
        if cand.exists() {
            if let Ok(canon) = cand.canonicalize() {
                return Ok(canon);
            }
        }
    }

    bail!(
        "verij_plugin.wasm not found. Build it with 'make build-plugin' or set VERIJ_PLUGIN_PATH."
    )
}

/// Resolves the command or binary path to invoke `verij` inside the sidebar pane.
pub fn resolve_verij_bin() -> String {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Ok(canon) = exe_path.canonicalize() {
            return canon.to_string_lossy().to_string();
        }
        return exe_path.to_string_lossy().to_string();
    }
    "verij".to_string()
}

/// Returns a cache directory path for generated Verij layouts.
pub fn get_cache_dir() -> Result<PathBuf> {
    crate::config::cache_dir().context("Cannot determine Verij cache directory")
}

// ---------------------------------------------------------------------------
// User-owned KDL template and runtime rendering
// ---------------------------------------------------------------------------

/// User-owned host template; independent of the source checkout at runtime.
pub fn user_layout_path() -> Result<PathBuf> {
    Ok(crate::config::config_path()
        .context("Cannot determine Verij config directory")?
        .with_file_name("verij.kdl"))
}

/// Install the bundled layout only when missing, preserving user edits.
pub fn init_user_layout() -> Result<PathBuf> {
    let path = user_layout_path()?;
    if init_user_layout_at(&path)? {
        eprintln!("[verij] Created host layout at {}", path.display());
    }
    Ok(path)
}

fn init_user_layout_at(path: &Path) -> Result<bool> {
    let parent = path.parent().context("Layout path has no parent")?;
    std::fs::create_dir_all(parent)?;
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(DEFAULT_LAYOUT.as_bytes()) {
                drop(file);
                let _ = std::fs::remove_file(path);
                return Err(error).with_context(|| format!("Failed to initialize {}", path.display()));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Failed to create {}", path.display())),
    }
}

/// Insert runtime values into an editable KDL template without changing that template.
pub fn render_layout(template: &str, config: &LayoutConfig) -> Result<String> {
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        rendered.push_str(&remaining[..start]);
        let placeholder = &remaining[start + 2..];
        let Some(end) = placeholder.find("}}") else {
            bail!("Unclosed Verij layout placeholder: {}", &remaining[start..]);
        };
        match &placeholder[..end] {
            "verij_bin" => rendered.push_str(&escape_kdl_string(&config.verij_bin)),
            "sidebar_width" => rendered.push_str(&escape_kdl_string(&config.sidebar_size)),
            key => bail!("Unknown Verij layout placeholder: {{{{{key}}}}}"),
        }
        remaining = &placeholder[end + 2..];
    }
    rendered.push_str(remaining);
    Ok(rendered)
}

fn escape_kdl_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Render user KDL into a cache file Zellij can load as a new host layout.
pub fn write_layout_file(config: &LayoutConfig, session_name: &str) -> Result<PathBuf> {
    crate::registry::validate_name(session_name)?;
    let path = init_user_layout()?;
    let template = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read user layout {}", path.display()))?;
    let content = render_layout(&template, config)?;
    let cache_dir = get_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create cache directory: {}", cache_dir.display()))?;

    let layout_file = cache_dir.join(format!("layout-{session_name}.kdl"));
    std::fs::write(&layout_file, content)
        .with_context(|| format!("Failed to write layout file: {}", layout_file.display()))?;

    Ok(layout_file)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_embedded_layout() {
        let config = LayoutConfig {
            verij_bin: "/usr/bin/verij".to_string(),
            sidebar_size: "30%".to_string(),
        };

        let kdl = render_layout(DEFAULT_LAYOUT, &config).unwrap();
        assert!(kdl.contains("tab name=\"Verij Host\""));
        assert!(kdl.contains("pane size=\"30%\" name=\"Verij\""));
        assert!(kdl.contains("command \"/usr/bin/verij\""));
        assert!(kdl.contains("args \"ui\""));
        assert!(kdl.contains("pane name=\"Workspace\" borderless=true"));
        assert!(!kdl.contains("plugin location="));
        assert!(kdl.contains("default_tab_template {"));
    }

    #[test]
    fn test_default_config() {
        let config = LayoutConfig::default();
        assert_eq!(config.sidebar_size, "25%");
    }

    #[test]
    fn test_resolve_plugin_path_explicit() {
        let dist_path = PathBuf::from("dist/verij_plugin.wasm");
        if dist_path.exists() {
            let res = resolve_plugin_path(Some(&dist_path));
            assert!(res.is_ok());
            let resolved = res.unwrap();
            assert!(resolved.is_absolute());
        }
    }

    #[test]
    fn test_init_layout_preserves_user_edits() {
        let path = std::env::temp_dir().join(format!(
            "verij-layout-test-{}-{}.kdl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(init_user_layout_at(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_LAYOUT);
        std::fs::write(&path, "pane size=\"{{sidebar_width}}\" name=\"Custom\"\n").unwrap();
        assert!(!init_user_layout_at(&path).unwrap());
        let edited = std::fs::read_to_string(&path).unwrap();
        assert_eq!(edited, "pane size=\"{{sidebar_width}}\" name=\"Custom\"\n");
        assert_eq!(render_layout(&edited, &LayoutConfig::default()).unwrap(),
            "pane size=\"25%\" name=\"Custom\"\n");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_template_escaping_and_unknown_placeholders() {
        let config = LayoutConfig {
            verij_bin: "some\\path\"quote".into(),
            sidebar_size: "20%".into(),
        };
        assert_eq!(
            render_layout("command \"{{verij_bin}}\"", &config).unwrap(),
            "command \"some\\\\path\\\"quote\"");
        assert!(render_layout("{{invalid}}", &config).is_err());
        let config = LayoutConfig {
            verij_bin: "{{sidebar_width}}".into(),
            sidebar_size: "20%".into(),
        };
        assert_eq!(render_layout("{{verij_bin}}", &config).unwrap(), "{{sidebar_width}}");
    }
}
