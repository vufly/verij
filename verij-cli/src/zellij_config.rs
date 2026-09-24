use crate::config::{Config, ZellijOption};
use anyhow::{bail, Context, Result};
use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const GENERATED_CONFIG_NAME: &str = "host-config.kdl";

/// Returns Zellij's configuration directory without requiring its config file to exist.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("ZELLIJ_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME").map(|home| PathBuf::from(home).join("zellij"))
        })
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/zellij")))
}

/// Returns the user-selected Zellij config if it exists.
///
/// An explicitly selected missing config is an error because silently falling back
/// would make host behavior differ from normal Zellij sessions.
pub fn effective_config_path() -> Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os("ZELLIJ_CONFIG_FILE").map(PathBuf::from) {
        if !path.exists() {
            bail!("ZELLIJ_CONFIG_FILE does not exist: {}", path.display());
        }
        return Ok(Some(path));
    }

    let Some(dir) = config_dir() else {
        return Ok(None);
    };
    let path = dir.join("config.kdl");
    if path.exists() {
        Ok(Some(path))
    } else if std::env::var_os("ZELLIJ_CONFIG_DIR").is_some() {
        bail!("ZELLIJ_CONFIG_DIR has no config.kdl: {}", path.display());
    } else {
        Ok(None)
    }
}

/// Render the complete host config without writing it.
pub fn render_host_config(config: &Config) -> Result<String> {
    let (base, source) = base_config()?;
    if let Some(source) = source {
        let generated = generated_config_path()?;
        if same_path(&source, &generated) {
            bail!(
                "Zellij base config resolves to Verij's generated file {}; unset ZELLIJ_CONFIG_FILE",
                generated.display()
            );
        }
    }
    patch_config(&base, &config.zellij.host_options())
}

/// Write and validate the host config cache, returning its absolute path.
pub fn write_host_config(config: &Config) -> Result<PathBuf> {
    let path = generated_config_path()?;
    let rendered = render_host_config(config)?;
    if std::fs::read_to_string(&path).ok().as_deref() == Some(&rendered) {
        return Ok(path);
    }

    let parent = path
        .parent()
        .context("Generated config path has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create Zellij config cache at {}",
            parent.display()
        )
    })?;
    let temporary = path.with_extension(format!("kdl.tmp-{}", std::process::id()));
    write_private_file(&temporary, &rendered)?;
    if let Err(error) = validate_config(&temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(&temporary, &path)
        .with_context(|| format!("Failed to replace generated config {}", path.display()))?;
    Ok(path)
}

pub fn generated_config_path() -> Result<PathBuf> {
    Ok(crate::config::cache_dir()
        .context("Cannot determine Verij cache directory")?
        .join(GENERATED_CONFIG_NAME))
}

fn base_config() -> Result<(String, Option<PathBuf>)> {
    match effective_config_path()? {
        Some(path) => {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read Zellij config {}", path.display()))?;
            Ok((content, Some(path)))
        }
        None => {
            let output = Command::new("zellij")
                .args(["setup", "--dump-config"])
                .output()
                .context("Failed to run `zellij setup --dump-config`")?;
            if !output.status.success() {
                bail!(
                    "`zellij setup --dump-config` failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            String::from_utf8(output.stdout)
                .map(|content| (content, None))
                .context("Zellij dumped non-UTF-8 configuration")
        }
    }
}

fn patch_config(base: &str, options: &BTreeMap<String, ZellijOption>) -> Result<String> {
    let mut document = base
        .parse::<KdlDocument>()
        .map_err(|error| anyhow::anyhow!("Invalid Zellij KDL: {error}"))?;

    for (name, value) in options {
        patch_option(&mut document, name, value)?;
    }

    Ok(document.to_string())
}

fn patch_option(document: &mut KdlDocument, name: &str, value: &ZellijOption) -> Result<()> {
    let nodes = document.nodes_mut();
    let matching = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (node.name().value() == name).then_some(index))
        .collect::<Vec<_>>();

    if let Some(&first) = matching.first() {
        let node = &mut nodes[first];
        if node.children().is_some() {
            bail!("Cannot override non-scalar Zellij config node '{name}'");
        }
        node.clear_entries();
        node.push(KdlEntry::new(kdl_value(value)));
        for index in matching.into_iter().skip(1).rev() {
            nodes.remove(index);
        }
    } else {
        let mut node = KdlNode::new(name);
        node.set_leading("\n");
        node.push(KdlEntry::new(kdl_value(value)));
        nodes.push(node);
    }
    Ok(())
}

fn kdl_value(value: &ZellijOption) -> KdlValue {
    match value {
        ZellijOption::Boolean(value) => (*value).into(),
        ZellijOption::Integer(value) => (*value).into(),
        ZellijOption::String(value) => value.clone().into(),
    }
}

fn validate_config(path: &Path) -> Result<()> {
    let output = Command::new("zellij")
        .args(["--config", &path.to_string_lossy(), "setup", "--check"])
        .output()
        .with_context(|| {
            format!(
                "Failed to validate generated Zellij config {}",
                path.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "Generated Zellij config is invalid:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn write_private_file(path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("Failed to create generated config {}", path.display()))?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

#[cfg(test)]
mod tests {
    use super::patch_config;
    use crate::config::ZellijOption;
    use std::collections::BTreeMap;

    #[test]
    fn patches_scalar_options_without_losing_other_config() {
        let base = "// Keep this comment\npane_frame_style \"full\"\nthemes {\n    custom {\n        fg 1\n    }\n}\nkeybinds {\n    normal {\n        bind \"x\" { Quit; }\n    }\n}\n";
        let options = BTreeMap::from([
            ("focus_follows_mouse".into(), ZellijOption::Boolean(true)),
            (
                "pane_frame_style".into(),
                ZellijOption::String("titles".into()),
            ),
            ("scroll_buffer_size".into(), ZellijOption::Integer(5000)),
        ]);

        let rendered = patch_config(base, &options).unwrap();

        assert!(rendered.contains("// Keep this comment"));
        assert!(rendered.contains("pane_frame_style \"titles\""));
        assert!(rendered.contains("focus_follows_mouse true"));
        assert!(rendered.contains("scroll_buffer_size 5000"));
        assert!(rendered.contains("themes {"));
        assert!(rendered.contains("custom {"));
        assert!(rendered.contains("bind \"x\" { Quit; }"));
    }

    #[test]
    fn removes_duplicate_option_nodes_and_escapes_strings() {
        let base = "pane_frame_style \"full\"\npane_frame_style \"none\"\n";
        let options = BTreeMap::from([(
            "pane_frame_style".into(),
            ZellijOption::String("title \"quoted\"".into()),
        )]);

        let rendered = patch_config(base, &options).unwrap();

        assert_eq!(rendered.matches("pane_frame_style").count(), 1);
        assert!(rendered.contains("title \\\"quoted\\\""));
    }

    #[test]
    fn rejects_non_scalar_override_target() {
        let base = "pane_frame_style { value \"full\" }\n";
        let options = BTreeMap::from([(
            "pane_frame_style".into(),
            ZellijOption::String("titles".into()),
        )]);

        assert!(patch_config(base, &options).is_err());
    }
}
