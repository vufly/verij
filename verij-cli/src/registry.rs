//! Durable host identity stored in XDG state, separate from attachment state.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Registry {
    pub hosts: BTreeMap<String, Host>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Host {
    pub marker_key: String,
}

pub fn path() -> Result<PathBuf> {
    crate::config::host_registry_path().context("Cannot determine Verij state directory")
}

pub fn load() -> Result<Registry> {
    let path = path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("Invalid {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(err) => Err(err).with_context(|| format!("Cannot read {}", path.display())),
    }
}

pub fn names() -> Result<HashSet<String>> {
    Ok(load()?.hosts.into_keys().collect())
}

pub fn marker_key(name: &str) -> Result<Option<String>> {
    Ok(load()?.hosts.get(name).map(|h| h.marker_key.clone()))
}

pub fn register(name: &str) -> Result<String> {
    validate_name(name)?;
    let mut registry = load()?;
    if let Some(host) = registry.hosts.get(name) {
        return Ok(host.marker_key.clone());
    }
    // Existing names remain valid across migration; generated UUID-like keys avoid
    // collisions with legacy marker names while keeping markers independent of renames.
    let key = format!("{}-{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?.as_nanos());
    registry.hosts.insert(name.to_string(), Host { marker_key: key.clone() });
    save(&registry)?;
    Ok(key)
}

pub fn rename(old: &str, new: &str) -> Result<()> {
    validate_name(new)?;
    let mut registry = load()?;
    if crate::config::load_hosts().hosts.contains_key(new) {
        bail!("Attachment state for '{new}' already exists");
    }
    registry.rename_entry(old, new)?;
    crate::config::rename_host_attachment(old, new)?;
    if let Err(error) = save(&registry) {
        let _ = crate::config::rename_host_attachment(new, old);
        return Err(error);
    }
    Ok(())
}

/// Remove registered hosts and/or orphan attachments as one maintenance operation.
/// Attachment writes are rolled back if the registry write fails.
pub fn remove_records(names: &[String]) -> Result<()> {
    let registry_path = path()?;
    let attachments_path = crate::config::hosts_path().context("Cannot determine host state path")?;
    let registry = load()?;
    let attachments = crate::config::load_hosts_checked()?;
    remove_records_at(&registry_path, &attachments_path, &crate::config::runtime_dir(),
        registry, attachments, names)
}

fn remove_records_at(
    registry_path: &Path,
    attachments_path: &Path,
    runtime_dir: &Path,
    mut registry: Registry,
    mut attachments: crate::config::HostsConfig,
    names: &[String],
) -> Result<()> {
    let original_attachments = attachments.clone();
    let mut marker_keys = Vec::new();
    let mut registry_changed = false;
    let mut attachments_changed = false;
    for name in names {
        if let Some(host) = registry.hosts.remove(name) {
            marker_keys.push(host.marker_key);
            registry_changed = true;
        }
        attachments_changed |= attachments.hosts.remove(name).is_some();
    }
    if attachments_changed {
        crate::config::write_hosts(attachments_path, &attachments)?;
    }
    if registry_changed {
        if let Err(error) = save_at(registry_path, &registry) {
            if attachments_changed {
                crate::config::write_hosts(attachments_path, &original_attachments)
                    .context("Failed to restore attachment state after registry write failure")?;
            }
            return Err(error);
        }
    }
    for key in marker_keys {
        let marker = runtime_dir.join(format!("workspace-{key}.session"));
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("Cannot remove {}", marker.display())),
        }
    }
    Ok(())
}

impl Registry {
    fn rename_entry(&mut self, old: &str, new: &str) -> Result<()> {
        if self.hosts.contains_key(new) {
            bail!("Host '{new}' is already registered");
        }
        let entry = self.hosts.remove(old).with_context(|| format!("Host '{old}' is not registered"))?;
        self.hosts.insert(new.to_string(), entry);
        Ok(())
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 || !name.chars().all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_') {
        bail!("Host name must be 1–64 alphanumeric, '-' or '_' characters");
    }
    Ok(())
}

fn save(registry: &Registry) -> Result<()> {
    let path = path()?;
    save_at(&path, registry)
}

fn save_at(path: &Path, registry: &Registry) -> Result<()> {
    let parent = path.parent().context("Registry has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
    std::fs::write(&temp, toml::to_string_pretty(registry)?)?;
    std::fs::rename(&temp, path).with_context(|| format!("Cannot replace {}", path.display()))
}

/// Avoid reparsing attachment TOML when only the last inner session changes.
pub struct Cache {
    modified: Option<std::time::SystemTime>,
    initialized: bool,
    pub names: HashSet<String>,
}

impl Cache {
    pub fn new() -> Result<Self> {
        let mut cache = Self { modified: None, initialized: false, names: HashSet::new() };
        cache.refresh()?;
        Ok(cache)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let file = path()?;
        let modified = match std::fs::metadata(&file) {
            Ok(meta) => meta.modified().ok(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        if modified != self.modified || !self.initialized {
            self.names = names()?;
            self.modified = modified;
            self.initialized = true;
        }
        Ok(())
    }
}

pub fn is_host_layout(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|layout| layout.contains("args \"ui\"") && layout.contains("Verij"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_validation() {
        assert!(validate_name("v").is_ok());
        assert!(validate_name("verij_v").is_ok());
        assert!(validate_name("../bad").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn legacy_record_is_readable() {
        let parsed: Registry = toml::from_str("[hosts.\"🔷v\"]\nmarker_key = \"🔷v\"\n").unwrap();
        assert_eq!(parsed.hosts["🔷v"].marker_key, "🔷v");
    }

    #[test]
    fn empty_host_registration_needs_no_attachment() {
        let registry: Registry = toml::from_str("[hosts.v]\nmarker_key = \"stable-1\"\n").unwrap();
        assert_eq!(registry.hosts["v"].marker_key, "stable-1");
        assert_eq!(registry.hosts.len(), 1);
    }

    #[test]
    fn rename_preserves_stable_marker_and_rejects_collision() {
        let mut registry = Registry::default();
        registry.hosts.insert("🔷v".into(), Host { marker_key: "🔷v".into() });
        registry.hosts.insert("other".into(), Host { marker_key: "different".into() });
        assert!(registry.rename_entry("🔷v", "other").is_err());
        assert_eq!(registry.hosts["🔷v"].marker_key, "🔷v");
        registry.rename_entry("🔷v", "v").unwrap();
        assert_eq!(registry.hosts["v"].marker_key, "🔷v");
        assert!(!registry.hosts.contains_key("🔷v"));
    }

    #[test]
    fn removal_cleans_host_and_attachment_state_without_touching_other_hosts() {
        let root = std::env::temp_dir().join(format!("verij-removal-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let registry_path = root.join("state/host-registry.toml");
        let attachment_path = root.join("state/hosts.toml");
        let runtime_dir = root.join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let mut registry = Registry::default();
        for (name, key) in [("gone", "stable-1"), ("keep", "stable-2")] {
            registry.hosts.insert(name.into(), Host { marker_key: key.into() });
            std::fs::write(runtime_dir.join(format!("workspace-{key}.session")), "inner").unwrap();
        }
        let mut attachments = crate::config::HostsConfig::default();
        for name in ["gone", "keep", "orphan"] {
            attachments.hosts.insert(name.into(), crate::config::HostAttachment { last_session: "inner".into() });
        }
        save_at(&registry_path, &registry).unwrap();
        crate::config::write_hosts(&attachment_path, &attachments).unwrap();

        remove_records_at(&registry_path, &attachment_path, &runtime_dir, registry, attachments,
            &["gone".into(), "orphan".into()]).unwrap();

        let saved: Registry = toml::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
        let saved_attachments: crate::config::HostsConfig =
            toml::from_str(&std::fs::read_to_string(&attachment_path).unwrap()).unwrap();
        assert_eq!(saved.hosts.keys().map(String::as_str).collect::<Vec<_>>(), ["keep"]);
        assert_eq!(saved_attachments.hosts.keys().map(String::as_str).collect::<Vec<_>>(), ["keep"]);
        assert!(!runtime_dir.join("workspace-stable-1.session").exists());
        assert!(runtime_dir.join("workspace-stable-2.session").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
