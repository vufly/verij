use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Layout configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub verij_bin: String,
    pub sidebar_size: String,
    pub sidebar_name: String,
    pub workspace_name: String,
    pub tab_name: String,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            verij_bin: "verij".to_string(),
            sidebar_size: "25%".to_string(),
            sidebar_name: "Verij".to_string(),
            workspace_name: "Workspace".to_string(),
            tab_name: "Verij Host".to_string(),
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
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(PathBuf::from(runtime_dir).join("verij"))
    } else if let Ok(cache_home) = std::env::var("XDG_CACHE_HOME") {
        Ok(PathBuf::from(cache_home).join("verij"))
    } else if let Ok(home) = std::env::var("HOME") {
        Ok(PathBuf::from(home).join(".cache").join("verij"))
    } else {
        Ok(std::env::temp_dir().join("verij"))
    }
}

// ---------------------------------------------------------------------------
// KDL generation
// ---------------------------------------------------------------------------

/// Generates a KDL layout string matching the Host Session structure.
pub fn generate_kdl(config: &LayoutConfig) -> String {
    format!(
        r#"layout {{
    default_tab_template {{
        children
    }}
    tab name="{tab_name}" {{
        pane split_direction="vertical" {{
            pane size="{sidebar_size}" name="{sidebar_name}" {{
                command "{verij_bin}"
                args "ui"
            }}
            pane name="{workspace_name}" borderless=true
        }}
    }}
}}
"#,
        tab_name = config.tab_name,
        sidebar_size = config.sidebar_size,
        sidebar_name = config.sidebar_name,
        verij_bin = config.verij_bin,
        workspace_name = config.workspace_name,
    )
}

/// Writes the generated KDL layout to the cache directory and returns its path.
pub fn write_layout_file(config: &LayoutConfig) -> Result<PathBuf> {
    let cache_dir = get_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create cache directory: {}", cache_dir.display()))?;

    let layout_file = cache_dir.join("layout.kdl");
    let content = generate_kdl(config);
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
    fn test_generate_kdl_output() {
        let config = LayoutConfig {
            verij_bin: "/usr/bin/verij".to_string(),
            sidebar_size: "30%".to_string(),
            sidebar_name: "Tree".to_string(),
            workspace_name: "Main".to_string(),
            tab_name: "Workspace Host".to_string(),
        };

        let kdl = generate_kdl(&config);
        assert!(kdl.contains("tab name=\"Workspace Host\""));
        assert!(kdl.contains("pane size=\"30%\" name=\"Tree\""));
        assert!(kdl.contains("command \"/usr/bin/verij\""));
        assert!(kdl.contains("args \"ui\""));
        assert!(kdl.contains("pane name=\"Main\" borderless=true"));
        assert!(!kdl.contains("plugin location="));
        assert!(kdl.contains("default_tab_template {"));
    }

    #[test]
    fn test_default_config() {
        let config = LayoutConfig::default();
        assert_eq!(config.sidebar_size, "25%");
        assert_eq!(config.sidebar_name, "Verij");
        assert_eq!(config.workspace_name, "Workspace");
        assert_eq!(config.tab_name, "Verij Host");
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
    fn test_write_layout_file() {
        let config = LayoutConfig {
            verij_bin: "/test/bin/verij".to_string(),
            sidebar_size: "20%".to_string(),
            sidebar_name: "Sidebar".to_string(),
            workspace_name: "Work".to_string(),
            tab_name: "Test Tab".to_string(),
        };

        let path = write_layout_file(&config).expect("write_layout_file should succeed");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read layout file");
        assert!(content.contains("pane size=\"20%\" name=\"Sidebar\""));
        assert!(content.contains("tab name=\"Test Tab\""));
    }
}
