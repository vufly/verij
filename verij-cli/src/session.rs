use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::path::Path;
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
pub fn start_host_session(session_name: &str, layout_path: &Path, no_attach: bool) -> Result<()> {
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

    let layout_str = layout_path.to_string_lossy();

    // Use -n (--new-session-with-layout) with -s to always create and attach to a new named session
    // with the given layout. In Zellij CLI, passing -l with -s treats it as adding tabs to an
    // existing session, which fails if the session does not already exist.
    exec_zellij(["-s", session_name, "-n", &layout_str])
}

/// Attaches to an existing host session.
pub fn attach_session(session_name: &str) -> Result<()> {
    if matches!(session_status(session_name)?, SessionStatus::Exited) {
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
        if matches!(session_status(session_name)?, SessionStatus::Live) {
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
    use super::{parse_session_status, SessionStatus};

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
}
