//! Owned attachment wrapper: publishes its real terminal context before exec.
use anyhow::{bail, Context, Result};
use kdl::{KdlDocument, KdlNode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use verij_types::identity::{valid_record_key, ProcessIdentity, TerminalPaneId};

#[derive(Serialize)]
struct Attachment {
    source: &'static str,
    host: String,
    host_session: String,
    workspace_pane: TerminalPaneId,
    attachment_process: ProcessIdentity,
}

pub fn control_dir() -> PathBuf {
    std::env::var_os("VERIJ_CONTROL_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| crate::fs_watcher::resolve_states_dir().with_file_name("control"))
}

pub fn attach(
    session: &str,
    host: Option<String>,
    plugin: Option<PathBuf>,
    client_config: Option<PathBuf>,
    force_run_commands: bool,
) -> Result<()> {
    let host_session = std::env::var("ZELLIJ_SESSION_NAME")
        .context("Workspace wrapper must run inside its host terminal")?;
    let host = host
        .or_else(|| std::env::var("VERIJ_HOST_MARKER_KEY").ok())
        .or(crate::registry::marker_key(&host_session)?)
        .context("host marker is not registered")?;
    if !valid_record_key(&host) {
        bail!("host marker cannot be used as a monitoring record key");
    }
    let pane = std::env::var("ZELLIJ_PANE_ID")?;
    let pane = pane
        .strip_prefix("terminal_")
        .unwrap_or(&pane)
        .parse::<u32>()?;
    let root = control_dir();
    crate::process::ensure_private_directory(&root)?;
    let config_path = prepare_config(&host, plugin, client_config)?;
    let attachment = Attachment {
        source: "owned_workspace_wrapper",
        host: host.clone(),
        host_session,
        workspace_pane: TerminalPaneId(pane),
        attachment_process: crate::process::identity(std::process::id())?,
    };
    write_private(
        &root.join(format!("attachment-{host}.json")),
        &serde_json::to_vec(&attachment)?,
    )?;
    let mut command = std::process::Command::new("zellij");
    command.args(["--config"]).arg(config_path).arg("attach");
    if force_run_commands {
        command.arg("--force-run-commands");
    }
    command.arg(session);
    for (name, _) in
        std::env::vars().filter(|(key, _)| key.starts_with("ZELLIJ") && key != "ZELLIJ_SOCKET_DIR")
    {
        command.env_remove(name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("failed to exec stock Workspace attachment")
    }
    #[cfg(not(unix))]
    {
        bail!("owned attachment exec unavailable on this platform")
    }
}

pub fn prepare_config(
    host: &str,
    plugin: Option<PathBuf>,
    client_config: Option<PathBuf>,
) -> Result<PathBuf> {
    if !valid_record_key(host) {
        bail!("invalid owned config host key");
    }
    let plugin = crate::layout::resolve_plugin_path(plugin.as_deref())?;
    let root = control_dir();
    crate::process::ensure_private_directory(&root)?;
    let base = match client_config.or(crate::zellij_config::effective_config_path()?) {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            let output = std::process::Command::new("zellij")
                .args(["setup", "--dump-config"])
                .output()?;
            if !output.status.success() {
                bail!("stock Zellij default config unavailable");
            }
            String::from_utf8(output.stdout)?
        }
    };
    let config = inject_registration(&base, &plugin)?;
    let config_path = root.join(format!("client-{host}.kdl"));
    write_private(&config_path, config.as_bytes())?;
    Ok(config_path)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(path.parent().context("missing file parent")?)?.sync_all()?;
    Ok(())
}

fn inject_registration(base: &str, plugin: &Path) -> Result<String> {
    let mut document: KdlDocument = base.parse()?;
    if document.get("keybinds").is_none() {
        document.nodes_mut().push(KdlNode::new("keybinds"));
    }
    let keys = document.get_mut("keybinds").unwrap();
    if keys.children().is_none() {
        keys.set_children(KdlDocument::new());
    }
    let keys = keys.children_mut().as_mut().unwrap();
    if keys.get("normal").is_none() {
        keys.nodes_mut().push(KdlNode::new("normal"));
    }
    let normal = keys.get_mut("normal").unwrap();
    if normal.children().is_none() {
        normal.set_children(KdlDocument::new());
    }
    let normal = normal.children_mut().as_mut().unwrap();
    normal.nodes_mut().retain(|node| {
        !(node.name().value() == "bind"
            && node.get(0).and_then(|entry| entry.value().as_string()) == Some("Ctrl b"))
    });
    let url = serde_json::to_string(&format!("file:{}", plugin.display()))?;
    let options = [
        ("VERIJ_WASI_CONTROL_DIR", "control_dir"),
        ("VERIJ_WASI_STATES_DIR", "state_dir"),
    ]
    .into_iter()
    .filter_map(|(env, key)| {
        std::env::var(env)
            .ok()
            .map(|path| format!("{key} {}\n", serde_json::to_string(&path).unwrap()))
    })
    .collect::<String>();
    let binding:KdlDocument=format!("bind \"Ctrl b\" {{\n LaunchOrFocusPlugin {url} {{\n floating true\n move_to_focused_tab true\n registration_surface true\n {options}}}\n}}\n").parse()?;
    normal.nodes_mut().extend(binding.nodes().iter().cloned());
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_overlay_preserves_unrelated_keys_and_replaces_only_registration_key() {
        let base="keybinds {\n normal {\n bind \"Ctrl b\" { Quit; }\n bind \"Alt x\" { NewPane; }\n }\n}\ntheme \"test\"\n";
        let config = inject_registration(base, Path::new("/tmp/verij plugin.wasm")).unwrap();
        assert!(config.contains("Alt x"));
        assert!(config.contains("theme \"test\""));
        assert_eq!(config.matches("bind \"Ctrl b\"").count(), 1);
        assert!(!config.contains("Quit"));
        assert!(config.contains("LaunchOrFocusPlugin"));
        assert!(config.contains("file:/tmp/verij plugin.wasm"));
    }
}
