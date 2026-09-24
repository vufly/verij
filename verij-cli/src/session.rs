use crate::config::ZellijOption;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

// ---------------------------------------------------------------------------
// Session queries
// ---------------------------------------------------------------------------

/// List all active Zellij session names.
pub fn list_sessions() -> Result<Vec<String>> {
    let output = match std::process::Command::new("zellij")
        .args(["list-sessions", "-s", "-n"])
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "'zellij' command not found in PATH. Please install Zellij (https://zellij.dev)."
            );
        }
        Err(e) => return Err(e).context("Failed to execute 'zellij list-sessions'"),
    };

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sessions = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(sessions)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Live,
    Exited,
    Missing,
}

/// Returns whether Zellij knows a session as live, exited/resurrectable, or missing.
pub fn session_status(name: &str) -> Result<SessionStatus> {
    let output = Command::new("zellij")
        .args(["list-sessions", "-n"])
        .output()
        .context("Failed to execute 'zellij list-sessions'")?;

    if !output.status.success() {
        return Ok(SessionStatus::Missing);
    }

    Ok(parse_session_status(
        &String::from_utf8_lossy(&output.stdout),
        name,
    ))
}

/// Returns the status of every session known to Zellij in one command.
pub fn session_statuses() -> Result<BTreeMap<String, SessionStatus>> {
    let output = Command::new("zellij")
        .args(["list-sessions", "-n"])
        .output()
        .context("Failed to execute 'zellij list-sessions'")?;

    if !output.status.success() {
        return Ok(BTreeMap::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.split_whitespace().next()?;
            let status = if line.contains("(EXITED") {
                SessionStatus::Exited
            } else {
                SessionStatus::Live
            };
            Some((name.to_string(), status))
        })
        .collect())
}

pub fn is_verij_host(name: &str) -> Result<bool> {
    if matches!(session_status(name)?, SessionStatus::Live) {
        let output = Command::new("zellij")
            .args(["--session", name, "action", "list-panes", "--all", "--json"])
            .output()?;
        if output.status.success() {
            let panes: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)?;
            return Ok(panes.iter().any(|pane| pane.get("pane_command")
                .or_else(|| pane.get("terminal_command"))
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("verij ui"))));
        }
    }
    let Some(layout) = resurrection_layout_path(name) else { return Ok(false); };
    Ok(crate::registry::is_host_layout(&layout))
}

pub fn rename_host(old: &str, new: &str) -> Result<()> {
    if old == new { return Ok(()); }
    crate::registry::validate_name(new)?;
    if !matches!(session_status(new)?, SessionStatus::Missing) {
        bail!("Zellij session '{new}' already exists");
    }
    if crate::registry::marker_key(old)?.is_none() {
        bail!("Host '{old}' is not registered");
    }
    if crate::registry::marker_key(new)?.is_some() || crate::config::load_hosts().hosts.contains_key(new) {
        bail!("Host '{new}' already has registered state");
    }
    if !matches!(session_status(old)?, SessionStatus::Live) {
        bail!("Host '{old}' must be live to rename; attach it first");
    }
    let status = Command::new("zellij")
        .args(["--session", old, "action", "rename-session", new])
        .status().context("Failed to invoke Zellij rename-session")?;
    if !status.success() { bail!("Zellij could not rename '{old}' to '{new}': {status}"); }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(session_status(new)?, SessionStatus::Live) { break; }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !matches!(session_status(new)?, SessionStatus::Live) {
        bail!("Zellij did not report renamed host '{new}'");
    }
    if let Err(error) = crate::registry::rename(old, new) {
        let _ = Command::new("zellij").args(["--session", new, "action", "rename-session", old]).status();
        return Err(error);
    }
    std::env::set_var("ZELLIJ_SESSION_NAME", new);
    std::env::set_var("VERIJ_HOST_NAME", new);
    Ok(())
}

// ---------------------------------------------------------------------------
// Process execution
// ---------------------------------------------------------------------------

/// Replaces the current process with Zellij, or spawns and waits if exec is unavailable.
pub fn exec_zellij<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = std::process::Command::new("zellij");
    cmd.args(args);

    #[cfg(unix)]
    {
        let err = cmd.exec();
        bail!("Failed to exec zellij: {}", err);
    }

    #[cfg(not(unix))]
    {
        let mut child = cmd.spawn().context("Failed to spawn zellij")?;
        let status = child.wait().context("Failed to wait for zellij")?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// High-level session actions
// ---------------------------------------------------------------------------

/// Launches the Verij host session.
///
/// If the session is already running and `no_attach` is false, automatically attaches.
pub fn start_host_session(
    session_name: &str,
    layout_path: &Path,
    no_attach: bool,
    zellij_options: &BTreeMap<String, ZellijOption>,
) -> Result<()> {
    match session_status(session_name)? {
        SessionStatus::Live | SessionStatus::Exited => {
            if no_attach {
                bail!(
                    "Session '{}' already exists. Specify a different name with --session-name or attach with 'verij attach'.",
                    session_name
                );
            }

            eprintln!("Session '{}' already exists. Attaching to it...", session_name);
            return attach_session(session_name);
        }
        SessionStatus::Missing => {}
    }

    // Use -n (--new-session-with-layout) with -s to always create and attach to a new named session
    // with the given layout. In Zellij CLI, passing -l with -s treats it as adding tabs to an
    // existing session, which fails if the session does not already exist.
    exec_zellij(host_start_args(session_name, layout_path, zellij_options))
}

fn host_start_args(
    session_name: &str,
    layout_path: &Path,
    zellij_options: &BTreeMap<String, ZellijOption>,
) -> Vec<String> {
    let mut args = vec![
        "-s".to_string(),
        session_name.to_string(),
        "-n".to_string(),
        layout_path.to_string_lossy().into_owned(),
        "options".to_string(),
    ];
    for (name, value) in zellij_options {
        args.push(format!("--{}", name.replace('_', "-")));
        args.push(value.to_string());
    }
    args
}

/// Attaches to an existing host session.
pub fn attach_session(session_name: &str) -> Result<()> {
    if matches!(session_status(session_name)?, SessionStatus::Exited) {
        let last_session = crate::config::last_host_session(session_name);
        prepare_resurrection_layout(session_name)?;
        prepare_host_workspace_layout(session_name, last_session.as_deref())?;
        exec_zellij(["attach", "--force-run-commands", session_name])
    } else {
        exec_zellij(["attach", session_name])
    }
}

/// Resurrects an exited session through a short-lived fake PTY, then detaches it.
/// This makes its panes and plugins available before a host Workspace pane attaches.
pub fn resurrect_session(session_name: &str) -> Result<()> {
    if !matches!(session_status(session_name)?, SessionStatus::Exited) {
        return Ok(());
    }

    prepare_resurrection_layout(session_name)?;

    let command = format!(
        "zellij attach --force-run-commands {}",
        shell_quote(session_name)
    );
    let mut fake_client = Command::new("script")
        .args(["-q", "-c", &command, "/dev/null"])
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to resurrect session '{session_name}'"))?;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(session_status(session_name)?, SessionStatus::Live)
            && session_has_visible_plugin(session_name)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let detach_status = Command::new("zellij")
        .args(["-s", session_name, "action", "detach"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to detach resurrection client")?;
    let _ = fake_client.wait();

    if !detach_status.success() {
        bail!("Failed to detach resurrection client for '{session_name}'");
    }
    if !matches!(session_status(session_name)?, SessionStatus::Live) {
        bail!("Session '{session_name}' did not become live after resurrection");
    }

    Ok(())
}

/// Resurrects the last inner session remembered by a host, when it still exists.
pub fn restore_last_inner_session(host_session: &str) -> Result<()> {
    let Some(inner_session) = crate::config::last_host_session(host_session) else {
        return Ok(());
    };

    resurrect_session(&inner_session)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"))
}

/// Removes Zellij's serialized `start_suspended` state before resurrection.
/// Zellij 0.45.1 can retain this flag even with `--force-run-commands`.
pub fn prepare_resurrection_layout(session_name: &str) -> Result<()> {
    let Some(path) = resurrection_layout_path(session_name) else {
        return Ok(());
    };
    let content = std::fs::read_to_string(&path)?;
    let updated = content.replace("start_suspended true", "start_suspended false");
    if updated == content {
        return Ok(());
    }

    let temporary = path.with_extension(format!("kdl.tmp-{}", std::process::id()));
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Rewrites a resurrected host's stale nested attach command to its durable target.
pub fn prepare_host_workspace_layout(host_session: &str, target_session: Option<&str>) -> Result<()> {
    let Some(target_session) = target_session else {
        return Ok(());
    };
    let Some(path) = resurrection_layout_path(host_session) else {
        return Ok(());
    };
    let content = std::fs::read_to_string(&path)?;
    let updated = rewrite_host_workspace_attach(&content, target_session);
    if updated == content {
        return Ok(());
    }

    let temporary = path.with_extension(format!("kdl.tmp-{}", std::process::id()));
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn resurrection_layout_path(session_name: &str) -> Option<PathBuf> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let path = cache_home
        .join("zellij/contract_version_1/session_info")
        .join(session_name)
        .join("session-layout.kdl");
    path.exists().then_some(path)
}

fn session_has_visible_plugin(session_name: &str) -> bool {
    let Ok(output) = Command::new("zellij")
        .args(["--session", session_name, "action", "list-panes", "--all", "--json"])
        .output()
    else {
        return false;
    };

    if !output.status.success() {
        return false;
    }

    let Ok(panes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
        return false;
    };

    panes.iter().any(|pane| {
        pane.get("is_plugin").and_then(|value| value.as_bool()) == Some(true)
            && pane.get("is_suppressed").and_then(|value| value.as_bool()) != Some(true)
    })
}

fn rewrite_host_workspace_attach(layout: &str, target_session: &str) -> String {
    let target = kdl_quote(target_session);
    let mut in_workspace_pane = false;
    let mut rewritten = Vec::new();

    for line in layout.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pane command=\"zellij\"") {
            in_workspace_pane = true;
        }

        if in_workspace_pane && trimmed.starts_with("args \"attach\"") {
            let indent = &line[..line.len() - trimmed.len()];
            rewritten.push(format!(
                "{indent}args \"attach\" \"--force-run-commands\" {target}"
            ));
            continue;
        }

        rewritten.push(line.to_string());
        if in_workspace_pane && trimmed == "}" {
            in_workspace_pane = false;
        }
    }

    let mut result = rewritten.join("\n");
    if layout.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn kdl_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn parse_session_status(output: &str, name: &str) -> SessionStatus {
    let prefix = format!("{name} ");
    output
        .lines()
        .map(str::trim)
        .find(|line| *line == name || line.starts_with(&prefix))
        .map(|line| {
            if line.contains("(EXITED") {
                SessionStatus::Exited
            } else {
                SessionStatus::Live
            }
        })
        .unwrap_or(SessionStatus::Missing)
}

#[cfg(test)]
mod tests {
    use super::{host_start_args, parse_session_status, SessionStatus};
    use crate::config::ZellijOption;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn parses_live_exited_and_missing_sessions() {
        let output = "backend [Created 1m ago]\nold [Created 2m ago] (EXITED - attach to resurrect)\n";

        assert_eq!(
            parse_session_status(output, "backend"),
            SessionStatus::Live
        );
        assert_eq!(parse_session_status(output, "old"), SessionStatus::Exited);
        assert_eq!(
            parse_session_status(output, "other"),
            SessionStatus::Missing
        );
    }

    #[test]
    fn does_not_match_session_name_prefixes() {
        let output = "backend-old [Created 1m ago]\n";

        assert_eq!(
            parse_session_status(output, "backend"),
            SessionStatus::Missing
        );
    }

    #[test]
    fn removes_serialized_suspended_state() {
        let layout = "pane command=\"watch\" start_suspended true\n";
        assert_eq!(
            layout.replace("start_suspended true", "start_suspended false"),
            "pane command=\"watch\" start_suspended false\n"
        );
    }

    #[test]
    fn rewrites_stale_host_workspace_attach() {
        let layout = "layout {\n    pane command=\"zellij\" {\n        args \"attach\" \"old\"\n    }\n}\n";

        assert_eq!(
            super::rewrite_host_workspace_attach(layout, "new"),
            "layout {\n    pane command=\"zellij\" {\n        args \"attach\" \"--force-run-commands\" \"new\"\n    }\n}\n"
        );
    }

    #[test]
    fn builds_host_start_options_from_sorted_config_entries() {
        let options = BTreeMap::from([
            (
                "focus_follows_mouse".to_string(),
                ZellijOption::Boolean(true),
            ),
            (
                "pane_frame_style".to_string(),
                ZellijOption::String("titles".to_string()),
            ),
            (
                "scroll_buffer_size".to_string(),
                ZellijOption::Integer(5000),
            ),
        ]);

        assert_eq!(
            host_start_args("host", Path::new("/tmp/host.kdl"), &options),
            vec![
                "-s",
                "host",
                "-n",
                "/tmp/host.kdl",
                "options",
                "--focus-follows-mouse",
                "true",
                "--pane-frame-style",
                "titles",
                "--scroll-buffer-size",
                "5000",
            ]
        );
    }
}
