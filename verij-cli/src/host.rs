//! Host management commands. The registry, not Zellij, defines which names are hosts.
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use crate::config;
use crate::registry::{self, Registry};
use crate::session::{self, SessionStatus};

fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Live => "live",
        SessionStatus::Exited => "exited",
        SessionStatus::Missing => "missing",
    }
}

fn status_of(statuses: &BTreeMap<String, SessionStatus>, name: &str) -> SessionStatus {
    statuses.get(name).copied().unwrap_or(SessionStatus::Missing)
}

pub fn list(live: bool, exited: bool, missing: bool) -> Result<()> {
    let registry = registry::load()?;
    let statuses = session::checked_session_statuses()?;
    let has_filter = live || exited || missing;
    for name in registry.hosts.keys() {
        let status = status_of(&statuses, name);
        let selected = !has_filter
            || (live && status == SessionStatus::Live)
            || (exited && status == SessionStatus::Exited)
            || (missing && status == SessionStatus::Missing);
        if selected {
            println!("{name}\t{}", status_label(status));
        }
    }
    Ok(())
}

pub fn show(name: &str, check: bool) -> Result<()> {
    if registry::marker_key(name)?.is_none() {
        bail!("Host '{name}' is not registered");
    }
    let statuses = check.then(session::checked_session_statuses).transpose()?;
    let attachments = config::load_hosts_checked()?;
    println!("Host: {name}");
    println!(
        "Last session: {}",
        attachments
            .hosts
            .get(name)
            .map(|attachment| attachment.last_session.as_str())
            .unwrap_or("none")
    );
    if let Some(statuses) = statuses {
        println!("Zellij: {}", status_label(status_of(&statuses, name)));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum DeleteAction {
    StateOnly,
    DeleteZellij { force: bool },
}

fn delete_action(status: SessionStatus, zellij: bool, force: bool) -> Result<DeleteAction> {
    if force && !zellij {
        bail!("--force requires --zellij");
    }
    match (status, zellij, force) {
        (SessionStatus::Missing, _, _) => Ok(DeleteAction::StateOnly),
        (SessionStatus::Live, true, false) => {
            bail!("Host is live; add --force to delete its Zellij session")
        }
        (SessionStatus::Live | SessionStatus::Exited, false, _) => {
            bail!("Host still exists in Zellij; use --zellij to delete it, or delete it in Zellij first")
        }
        (_, true, force) => Ok(DeleteAction::DeleteZellij { force }),
    }
}

pub fn delete(name: &str, zellij: bool, force: bool) -> Result<()> {
    if registry::marker_key(name)?.is_none() {
        bail!("Host '{name}' is not registered");
    }
    let status = status_of(&session::checked_session_statuses()?, name);
    if status != SessionStatus::Missing
        && std::env::var("ZELLIJ_SESSION_NAME").as_deref() == Ok(name)
    {
        bail!("Cannot delete host '{name}' from inside itself; run this command outside that session");
    }
    match delete_action(status, zellij, force)? {
        DeleteAction::StateOnly => {
            if status_of(&session::checked_session_statuses()?, name) != SessionStatus::Missing {
                bail!("Zellij session '{name}' appeared during deletion; retry");
            }
        }
        DeleteAction::DeleteZellij { force } => {
            if !session::is_verij_host_for_deletion(name)? {
                bail!("Zellij session '{name}' is not verified as a Verij host; refusing to delete it");
            }
            // Do not force-delete if a previously exited host became live, or if
            // a live host was replaced while inspecting its panes.
            let current = status_of(&session::checked_session_statuses()?, name);
            if current != status {
                bail!("Zellij session '{name}' changed status during deletion; retry");
            }
            let mut cmd = Command::new("zellij");
            cmd.arg("delete-session");
            if force {
                cmd.arg("--force");
            }
            let status = cmd
                .arg(name)
                .status()
                .context("Failed to execute 'zellij delete-session'")?;
            if !status.success() {
                bail!("Zellij could not delete session '{name}': {status}");
            }
            if status_of(&session::checked_session_statuses()?, name) != SessionStatus::Missing {
                bail!("Zellij still lists session '{name}'; Verij state was preserved");
            }
        }
    }
    registry::remove_records(&[name.to_owned()])?;
    println!("Deleted host '{name}'");
    Ok(())
}

fn prune_candidates(
    registry: &Registry,
    attachments: &config::HostsConfig,
    statuses: &BTreeMap<String, SessionStatus>,
) -> Vec<String> {
    registry
        .hosts
        .keys()
        .chain(attachments.hosts.keys())
        .filter(|name| status_of(statuses, name) == SessionStatus::Missing)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn prune(dry_run: bool) -> Result<()> {
    let registry = registry::load()?;
    let attachments = config::load_hosts_checked()?;
    let statuses = session::checked_session_statuses()?;
    let mut candidates = prune_candidates(&registry, &attachments, &statuses);
    if !dry_run && !candidates.is_empty() {
        // A host may have been created since the first inventory was read.
        let current = session::checked_session_statuses()?;
        candidates.retain(|name| status_of(&current, name) == SessionStatus::Missing);
        registry::remove_records(&candidates)?;
    }
    for name in &candidates {
        println!("{} {name}", if dry_run { "Would prune" } else { "Pruned" });
    }
    println!(
        "{} host record(s){}",
        candidates.len(),
        if dry_run { " would be pruned" } else { " pruned" }
    );
    Ok(())
}

pub fn rename(old: &str, new: &str) -> Result<()> {
    session::rename_host(old, new)?;
    println!("Renamed host '{old}' to '{new}'");
    Ok(())
}

pub fn recover(name: Option<&str>) -> Result<()> {
    let name = match name {
        Some(name) => name.to_owned(),
        None => std::env::var("ZELLIJ_SESSION_NAME")
            .context("Recover requires a host name outside a Zellij host session")?,
    };
    let marker_key = registry::marker_key(&name)?
        .with_context(|| format!("Host '{name}' is not registered"))?;
    if !matches!(session::session_status(&name)?, SessionStatus::Live) {
        bail!("Host '{name}' must be live to recover its layout");
    }

    let config = config::Config::load();
    let layout = crate::layout::write_layout_file(
        &crate::layout::LayoutConfig {
            verij_bin: crate::layout::resolve_verij_bin(),
            sidebar_size: config.workspace.sidebar_width,
        },
        &name,
    )?;
    let marker = config::runtime_dir().join(format!("workspace-{marker_key}.session"));
    let previous_marker = match std::fs::read(&marker) {
        Ok(content) => {
            std::fs::remove_file(&marker)
                .with_context(|| format!("Cannot clear Workspace marker for host '{name}'"))?;
            Some(content)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("Cannot read Workspace marker for host '{name}'")),
    };

    let status = match Command::new("zellij")
        .args(["--session", &name, "action", "override-layout"])
        .arg(&layout)
        .status()
    {
        Ok(status) => status,
        Err(error) => {
            if let Some(content) = &previous_marker {
                std::fs::write(&marker, content)
                    .context("Failed to restore Workspace marker after layout recovery failed")?;
            }
            return Err(error).context("Failed to execute 'zellij action override-layout'");
        }
    };
    if !status.success() {
        if let Some(content) = &previous_marker {
            std::fs::write(&marker, content)
                .context("Failed to restore Workspace marker after layout recovery failed")?;
        }
        bail!("Zellij could not recover host '{name}' layout: {status}");
    }

    println!("Recovered host '{name}' layout");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HostAttachment;
    use crate::registry::Host;

    #[test]
    fn deletion_requires_explicit_zellij_and_force_for_live() {
        assert_eq!(delete_action(SessionStatus::Missing, false, false).unwrap(), DeleteAction::StateOnly);
        assert_eq!(delete_action(SessionStatus::Missing, true, false).unwrap(), DeleteAction::StateOnly);
        assert!(delete_action(SessionStatus::Exited, false, false).is_err());
        assert_eq!(delete_action(SessionStatus::Exited, true, false).unwrap(), DeleteAction::DeleteZellij { force: false });
        assert!(delete_action(SessionStatus::Live, true, false).is_err());
        assert_eq!(delete_action(SessionStatus::Live, true, true).unwrap(), DeleteAction::DeleteZellij { force: true });
        assert!(delete_action(SessionStatus::Missing, false, true).is_err());
    }

    #[test]
    fn prune_keeps_live_and_resurrectable_hosts_but_removes_missing_and_orphans() {
        let mut registry = Registry::default();
        for name in ["live", "exited", "missing"] {
            registry.hosts.insert(name.into(), Host { marker_key: name.into() });
        }
        let mut attachments = config::HostsConfig::default();
        for name in ["live", "missing", "orphan", "live-orphan"] {
            attachments.hosts.insert(name.into(), HostAttachment { last_session: "inner".into() });
        }
        let statuses = BTreeMap::from([
            ("live".into(), SessionStatus::Live),
            ("exited".into(), SessionStatus::Exited),
            ("live-orphan".into(), SessionStatus::Live),
        ]);
        assert_eq!(prune_candidates(&registry, &attachments, &statuses), ["missing", "orphan"]);
        assert_eq!(status_label(status_of(&statuses, "missing")), "missing");
    }
}
