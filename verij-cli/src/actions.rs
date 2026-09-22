/// verij-cli/src/actions.rs
///
/// Action dispatcher: translates TUI navigation selections into Zellij shell
/// commands executed via `std::process::Command`.
///
/// Design rationale:
///   - We use `zellij action` CLI subcommands rather than the plugin pipe
///     because the TUI process is a native binary with direct shell access.
///   - All commands are fire-and-forget (we don't wait for confirmation).
///     Zellij applies them asynchronously; the next `SessionUpdate` event from
///     the plugin will reflect any state changes.
use anyhow::{Context, Result};
use std::process::Command;

// ---------------------------------------------------------------------------
// Public action API
// ---------------------------------------------------------------------------

/// Focus the named Zellij session (attaches it to Workspace pane if needed, goes to first tab).
pub fn switch_session(session_name: &str) -> Result<()> {
    switch_session_tab(session_name, 0)
}

/// Switch focus to a specific tab within the named session.
///
/// If the target session is already attached in the Host Session's Workspace pane,
/// this simply sends `go-to-tab` to the target session.
///
/// If a different session is attached (or none), it cleanly detaches the existing session,
/// attaches the target session into the Workspace pane, and switches to the requested tab.
///
/// This does NOT switch the host client connection (no fullscreen takeover);
/// the host session layout remains intact while the inner session updates in-place.
pub fn switch_session_tab(target_session: &str, tab_position: usize) -> Result<()> {
    let host_session = std::env::var("ZELLIJ_SESSION_NAME").ok();

    // If target is host session itself, just navigate tabs in host session
    if let Some(ref host) = host_session {
        if target_session == host {
            let tab_arg = (tab_position + 1).to_string();
            return run_zellij_action_for_session(Some(target_session), &["go-to-tab", &tab_arg])
                .with_context(|| format!("Failed to go to tab {tab_position} in session `{target_session}`"));
        }
    }

    let ws_state = detect_workspace_state(host_session.as_deref());

    if ws_state.attached_session.as_deref() == Some(target_session) {
        // Session is already attached in workspace pane. Just switch its tab!
        let tab_arg = (tab_position + 1).to_string();
        run_zellij_action_for_session(Some(target_session), &["go-to-tab", &tab_arg])
            .with_context(|| format!("Failed to go to tab {tab_position} in session `{target_session}`"))?;
    } else {
        // Different session needs to be attached into the workspace pane!
        if let Some(pid) = ws_state.attach_pid {
            // Detach previous session cleanly by sending SIGTERM to zellij attach
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();

            // Wait briefly for process to exit
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    break;
                }
            }
        }

        // Focus the Workspace pane BEFORE attaching so the nested session prompt receives input
        let _ = run_zellij_action_for_session(
            host_session.as_deref(),
            &["focus-pane-id", &ws_state.workspace_pane_id],
        );
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Attach target session inside Workspace pane
        let attach_cmd = format!("zellij attach {}\n", target_session);
        run_zellij_action_for_session(
            host_session.as_deref(),
            &["write-chars", "--pane-id", &ws_state.workspace_pane_id, &attach_cmd],
        ).with_context(|| format!("Failed to attach `{target_session}` in workspace pane"))?;

        // If a specific non-first tab is requested, navigate after attach connects
        if tab_position > 0 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let tab_arg = (tab_position + 1).to_string();
            let _ = run_zellij_action_for_session(Some(target_session), &["go-to-tab", &tab_arg]);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Run `zellij [--session <name>] action <args>` and wait for it to exit.
///
/// Stdout and stderr are captured to prevent corrupting the TUI alternate screen.
fn run_zellij_action_for_session(session_name: Option<&str>, args: &[&str]) -> Result<()> {
    let mut cmd_args = Vec::new();
    if let Some(session) = session_name {
        cmd_args.push("--session");
        cmd_args.push(session);
    }
    cmd_args.push("action");
    cmd_args.extend_from_slice(args);

    let output = Command::new("zellij")
        .args(&cmd_args)
        .output()
        .context("Failed to spawn `zellij` process")?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        anyhow::bail!(
            "`zellij {}` failed (status {}): {}",
            cmd_args.join(" "),
            output.status,
            stderr_str.trim()
        );
    }

    if stdout_str.contains("not found") {
        anyhow::bail!("{}", stdout_str.lines().next().unwrap_or("Session not found"));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Workspace process inspection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct WorkspaceState {
    pub attached_session: Option<String>,
    pub attach_pid: Option<u32>,
    pub workspace_pane_id: String,
}

/// Detect what session (if any) is running in the Host Session's Workspace pane.
pub fn detect_workspace_state(host_session: Option<&str>) -> WorkspaceState {
    let mut state = WorkspaceState {
        attached_session: None,
        attach_pid: None,
        workspace_pane_id: "terminal_1".to_string(),
    };

    // Find workspace pane id from `zellij action list-panes`
    if let Some(host) = host_session {
        if let Ok(output) = Command::new("zellij")
            .args(["--session", host, "action", "list-panes"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("Workspace") {
                    if let Some(pane_id) = line.split_whitespace().next() {
                        state.workspace_pane_id = pane_id.to_string();
                        break;
                    }
                }
            }
        }
    }

    // Inspect Linux /proc to find child running inside Workspace pane
    #[cfg(target_os = "linux")]
    {
        if let Some((session, pid)) = inspect_linux_workspace_process() {
            state.attached_session = Some(session);
            state.attach_pid = Some(pid);
        }
    }

    state
}

#[cfg(target_os = "linux")]
fn inspect_linux_workspace_process() -> Option<(String, u32)> {
    let my_pid = std::process::id();
    let my_stat = std::fs::read_to_string(format!("/proc/{my_pid}/stat")).ok()?;
    let host_ppid = parse_ppid_from_stat(&my_stat)?;

    let mut sibling_pids = Vec::new();
    let proc_dir = std::fs::read_dir("/proc").ok()?;

    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(pid) = name_str.parse::<u32>() {
            if pid == my_pid {
                continue;
            }
            if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
                if let Some(ppid) = parse_ppid_from_stat(&stat) {
                    if ppid == host_ppid {
                        sibling_pids.push(pid);
                    }
                }
            }
        }
    }

    if sibling_pids.is_empty() {
        return None;
    }

    // Scan for children of sibling panes
    let proc_dir = std::fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(pid) = name_str.parse::<u32>() {
            if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
                if let Some(ppid) = parse_ppid_from_stat(&stat) {
                    if sibling_pids.contains(&ppid) {
                        // Direct child of sibling pane
                        if let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) {
                            if let Some(session) = parse_session_from_cmdline(&cmdline) {
                                return Some((session, pid));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn parse_ppid_from_stat(stat: &str) -> Option<u32> {
    let last_paren = stat.rfind(')')?;
    let after_paren = stat.get(last_paren + 1..)?;
    let mut parts = after_paren.split_whitespace();
    let _state = parts.next()?;
    let ppid_str = parts.next()?;
    ppid_str.parse::<u32>().ok()
}

#[cfg(target_os = "linux")]
fn parse_session_from_cmdline(cmdline: &[u8]) -> Option<String> {
    let args: Vec<String> = cmdline
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect();

    if args.is_empty() {
        return None;
    }

    let first = args.first()?;
    if !first.contains("zellij") {
        return None;
    }

    // Check for -s or --session
    for i in 0..args.len() {
        if (args[i] == "-s" || args[i] == "--session") && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }

    // Check for attach / a
    if let Some(pos) = args.iter().position(|arg| arg == "attach" || arg == "a") {
        for arg in args.iter().skip(pos + 1).rev() {
            if !arg.starts_with('-') {
                return Some(arg.clone());
            }
        }
    }

    None
}

