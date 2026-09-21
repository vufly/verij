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

/// Switch focus to the named Zellij session.
///
/// Uses `zellij action switch-session <name>`, which attaches the named
/// session in the current client's terminal.
///
/// In the Verij nested-session model this will attach the Inner Session into
/// whatever pane the Host Session client is currently focused on (the right
/// pane by convention).
pub fn switch_session(session_name: &str) -> Result<()> {
    run_zellij_action(&["switch-session", session_name])
        .with_context(|| format!("Failed to switch to session `{session_name}`"))
}

/// Switch focus to a specific tab within the named session.
///
/// First switches to the session, then navigates to the tab by its 1-based
/// position (Zellij tab positions are 0-based internally; `go-to-tab` takes
/// 1-based indices).
pub fn switch_session_tab(session_name: &str, tab_position: usize) -> Result<()> {
    // Switch to the session first.
    switch_session(session_name)?;

    // Then navigate to the specific tab.
    // `go-to-tab` expects a 1-based index.
    let tab_arg = (tab_position + 1).to_string();
    run_zellij_action(&["go-to-tab", &tab_arg])
        .with_context(|| format!("Failed to go to tab {tab_position} in `{session_name}`"))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Run `zellij action <args>` and wait for it to exit.
///
/// stdout and stderr are inherited (displayed in the terminal) so that any
/// error messages from Zellij are visible to the user.
fn run_zellij_action(args: &[&str]) -> Result<()> {
    let mut cmd_args = vec!["action"];
    cmd_args.extend_from_slice(args);

    let status = Command::new("zellij")
        .args(&cmd_args)
        .status()
        .context("Failed to spawn `zellij` process")?;

    if !status.success() {
        anyhow::bail!(
            "`zellij action {}` exited with status: {}",
            args.join(" "),
            status
        );
    }

    Ok(())
}
