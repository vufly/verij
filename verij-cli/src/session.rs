use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::path::Path;

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

/// Checks if a session with the given name is currently active.
pub fn is_session_running(name: &str) -> Result<bool> {
    let sessions = list_sessions()?;
    Ok(sessions.iter().any(|s| s == name))
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
    if is_session_running(session_name)? {
        if no_attach {
            bail!(
                "Session '{}' is already running. Specify a different name with --session-name or attach with 'verij attach'.",
                session_name
            );
        } else {
            eprintln!(
                "Session '{}' is already running. Attaching to it...",
                session_name
            );
            return attach_session(session_name);
        }
    }

    let layout_str = layout_path.to_string_lossy();

    // If currently inside an existing Zellij session, use -n to force creating a new session
    // instead of appending tabs to the current session.
    let is_inside_zellij = std::env::var("ZELLIJ").is_ok();

    if is_inside_zellij {
        exec_zellij(["-s", session_name, "-n", &layout_str])
    } else {
        exec_zellij(["-s", session_name, "-l", &layout_str])
    }
}

/// Attaches to an existing host session.
pub fn attach_session(session_name: &str) -> Result<()> {
    exec_zellij(["attach", session_name])
}
