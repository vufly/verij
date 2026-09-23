/// verij-cli/src/actions.rs
///
/// Action dispatcher: handles session and tab switching using "The Inception Switch".
///
/// Design rationale:
///   - Session switching: Injects a `switch:<target>` command into the
///     *currently active inner session* via `zellij -s <old_session> pipe --name verij_control`.
///     The `verij-plugin` agent running inside that session executes native
///     `switch_session(Some(target))`, switching the attached client cleanly
///     from the inside.
///   - Re-focus: Immediately executes `zellij action move-focus right` so keyboard
///     focus transfers to the attached inner session.
///   - Tab navigation: Directly executes `zellij --session <target> action go-to-tab <pos+1>`.
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use verij_types::VERIJ_CONTROL_PIPE;

/// Creates a new inner session in the background, ensures the WASM agent is running,
/// and attaches/switches the Workspace pane to it.
pub fn create_inner_session(
    old_active_session: Option<&str>,
    new_session_name: &str,
    plugin_path: Option<&Path>,
) -> Result<()> {
    // 1. Create the session with a fake PTY via `script` so zellij has a valid viewport.
    //
    // Zellij 0.45.x regression (zellij-org/zellij#5594): tabs created against a session
    // with no attached client have no viewport, so `default_tab_template` layout fails with
    // "Not enough room for panes" and the tab (+ zjstatus pane) is silently discarded when
    // the first real client attaches. Using -b (headless) triggers this every time.
    //
    // Fix: attach through `script -q -c "zellij attach -c <name>" /dev/null` which provides
    // a pseudo-terminal, giving zellij a viewport to size the first tab from. After the
    // layout settles we send `zellij action detach` to that fake client and wait for the
    // `script` process to exit cleanly before proceeding.
    let default_layout = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config/zellij/layouts/default.kdl"))
        .ok()
        .filter(|p| p.exists());

    // Build the inner zellij command that `script` will run in its fake PTY.
    let mut zellij_args = vec![
        "zellij".to_string(),
        "attach".to_string(),
        "-c".to_string(),
        new_session_name.to_string(),
    ];
    if let Some(ref layout) = default_layout {
        zellij_args.extend([
            "options".to_string(),
            "--default-layout".to_string(),
            layout.to_string_lossy().into_owned(),
        ]);
    }
    let zellij_cmd = zellij_args.join(" ");

    let mut fake_client = Command::new("script")
        .args(["-q", "-c", &zellij_cmd, "/dev/null"])
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| {
            format!("Failed to create session '{new_session_name}' via fake-PTY attach")
        })?;

    // Wait for zellij to apply the default_tab_template layout inside the fake PTY.
    std::thread::sleep(std::time::Duration::from_millis(400));

    // Detach the fake client — the session stays alive, layout already applied.
    let _ = Command::new("zellij")
        .args(["-s", new_session_name, "action", "detach"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Wait for `script` to exit after zellij detaches (prevents zombie processes).
    let _ = fake_client.wait();

    // Set inner session pane frame style to full
    let _ = Command::new("zellij")
        .args(["-s", new_session_name, "action", "set-pane-frame-style", "full"])
        .stdin(std::process::Stdio::null())
        .status();

    // 2. Check if agent state file appears (auto-loaded via Zellij load_plugins).
    // If not after brief delay, explicitly launch plugin with --floating --no-focus fallback.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let state_file =
        std::path::PathBuf::from(format!("/tmp/verij/states/{new_session_name}.json"));
    if !state_file.exists() {
        if let Some(path) = plugin_path {
            let plugin_url = format!("file:{}", path.display());
            let _ = Command::new("zellij")
                .args([
                    "-s",
                    new_session_name,
                    "action",
                    "launch-plugin",
                    "--floating",
                    "--no-focus",
                    &plugin_url,
                ])
                .status();
        }
    }

    // 3. Switch right pane to this new session
    switch_session(old_active_session, new_session_name, None)?;

    Ok(())
}

/// Switch to the target session and optionally navigate to a specific tab.
///
/// If `old_active_session` is present and differs from `target_session`,
/// triggers the Inception Switch via pipe injection into `old_active_session`.
/// If no session was previously active (e.g. initial launch in empty pane),
/// falls back to attaching directly via `zellij action write-chars`.
pub fn switch_session(
    old_active_session: Option<&str>,
    target_session: &str,
    tab_position: Option<usize>,
) -> Result<()> {
    match old_active_session {
        Some(old) if old == target_session => {
            // Target is already attached. Just navigate tab if requested.
            if let Some(pos) = tab_position {
                switch_tab(target_session, pos)?;
            }
        }
        Some(old) => {
            // The Inception Switch: trigger switch from inside the current session
            inception_switch(old, target_session)?;

            // If a specific tab is requested, navigate after brief delay
            if let Some(pos) = tab_position {
                if pos > 0 {
                    let target = target_session.to_string();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        let _ = switch_tab(&target, pos);
                    });
                }
            }
        }
        None => {
            // First attach fallback: attach into the right pane
            attach_in_right_pane(target_session)?;

            if let Some(pos) = tab_position {
                if pos > 0 {
                    let target = target_session.to_string();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let _ = switch_tab(&target, pos);
                    });
                }
            }
        }
    }

    // Ensure inner session ready before refocusing
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Always re-focus the right pane so keyboard input goes to the attached workspace
    re_focus_right_pane()?;

    Ok(())
}

/// Executes "The Inception Switch":
/// `zellij -s <old_session> pipe --name verij_control -- switch:<new_session>`
pub fn inception_switch(old_active_session: &str, new_selected_session: &str) -> Result<()> {
    let payload = format!("switch:{}", new_selected_session);

    let status = Command::new("zellij")
        .args([
            "-s",
            old_active_session,
            "pipe",
            "--name",
            VERIJ_CONTROL_PIPE,
            "--",
            &payload,
        ])
        .status()
        .with_context(|| {
            format!(
                "Failed to dispatch Inception Switch pipe to session '{}'",
                old_active_session
            )
        })?;

    if !status.success() {
        eprintln!(
            "[verij-cli] Warning: Inception Switch command exited with status: {}",
            status
        );
    }

    Ok(())
}

/// Re-focuses the right pane: `zellij action move-focus right`.
pub fn re_focus_right_pane() -> Result<()> {
    let status = Command::new("zellij")
        .args(["action", "move-focus", "right"])
        .status()
        .context("Failed to execute `zellij action move-focus right`")?;

    if !status.success() {
        eprintln!(
            "[verij-cli] Warning: `zellij action move-focus right` exited with status: {}",
            status
        );
    }

    Ok(())
}

/// Switch to a specific tab within the named session:
/// `zellij --session <target> action go-to-tab <position + 1>`.
pub fn switch_tab(session_name: &str, tab_position: usize) -> Result<()> {
    let tab_arg = (tab_position + 1).to_string();
    let status = Command::new("zellij")
        .args(["--session", session_name, "action", "go-to-tab", &tab_arg])
        .status()
        .with_context(|| {
            format!(
                "Failed to navigate to tab {} in session '{}'",
                tab_position, session_name
            )
        })?;

    if !status.success() {
        eprintln!(
            "[verij-cli] Warning: `zellij action go-to-tab` exited with status: {}",
            status
        );
    }

    Ok(())
}

/// Initial attach fallback when no session was previously active in the right pane:
/// sends `stty sane; zellij attach <target>\n` to the active pane.
fn attach_in_right_pane(target_session: &str) -> Result<()> {
    let attach_cmd = format!("stty sane; zellij attach {}\n", target_session);
    let _ = Command::new("zellij")
        .args(["action", "move-focus", "right"])
        .stdin(std::process::Stdio::null())
        .status();

    let status = Command::new("zellij")
        .args(["action", "write-chars", &attach_cmd])
        .stdin(std::process::Stdio::null())
        .status()
        .with_context(|| {
            format!(
                "Failed to send initial attach command for session '{}'",
                target_session
            )
        })?;

    if !status.success() {
        eprintln!(
            "[verij-cli] Warning: initial attach write-chars exited with status: {}",
            status
        );
    }

    Ok(())
}
