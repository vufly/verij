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
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use verij_types::VERIJ_CONTROL_PIPE;

/// Creates a new inner session in the background, ensures the WASM agent is running,
/// and attaches/switches the Workspace pane to it.
pub fn create_inner_session(
    old_active_session: Option<&str>,
    new_session_name: &str,
    plugin_path: Option<&Path>,
    workspace_pane_name: &str,
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

    // Wait until the default layout's visible plugin pane is ready.
    // A fixed delay can detach before a layout plugin finishes loading.
    if default_layout.is_some() {
        wait_for_layout_plugin(new_session_name);
    } else {
        std::thread::sleep(Duration::from_millis(400));
    }

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

    // 3. Switch right pane to this new session and rename the Workspace pane
    switch_session(
        old_active_session,
        new_session_name,
        None,
        Some(workspace_pane_name),
    )?;

    Ok(())
}

/// Switch to the target session and optionally navigate to a specific tab.
///
/// If `old_active_session` is present and differs from `target_session`,
/// triggers the Inception Switch via pipe injection into `old_active_session`.
/// If no session was previously active (e.g. initial launch in empty pane),
/// falls back to attaching directly via `zellij action write-chars`.
///
/// `workspace_pane_name`: if `Some(name)`, renames the host session's Workspace pane
/// to `name` after switching. Pass `None` to skip renaming.
pub fn switch_session(
    old_active_session: Option<&str>,
    target_session: &str,
    tab_position: Option<usize>,
    workspace_pane_name: Option<&str>,
) -> Result<()> {
    match old_active_session {
        Some(old) if old == target_session => {
            match tab_position {
                Some(pos) => {
                    // Tab click on the currently active session: just navigate the tab.
                    // The right pane should already be attached; switch_tab is safe.
                    switch_tab(target_session, pos)?;
                }
                None => {
                    // `active_session` identifies the Workspace attachment. Do not
                    // write another attach command into an already attached pane.
                }
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

    // Rename the right-side Workspace pane, not the host session tab.
    if let Some(name) = workspace_pane_name {
        rename_workspace_pane(name)?;
    }
    set_workspace_session(target_session)?;

    Ok(())
}

const WORKSPACE_SESSION_ENV: &str = "VERIJ_WORKSPACE_SESSION";

fn workspace_marker_path() -> Option<PathBuf> {
    let host_session = std::env::var("ZELLIJ_SESSION_NAME").ok()?;
    Some(
        std::env::temp_dir()
            .join("verij")
            .join(format!("workspace-{host_session}.session")),
    )
}

/// Returns session recorded as attached to this host Workspace pane.
pub fn workspace_session() -> Option<String> {
    if let Some(path) = workspace_marker_path() {
        return std::fs::read_to_string(path)
            .ok()
            .map(|session| session.trim().to_string())
            .filter(|session| !session.is_empty());
    }
    std::env::var(WORKSPACE_SESSION_ENV)
        .ok()
        .filter(|session| !session.is_empty())
}

/// Records the inner session currently attached to the host Workspace pane.
pub fn set_workspace_session(session: &str) -> Result<()> {
    std::env::set_var(WORKSPACE_SESSION_ENV, session);
    let Some(path) = workspace_marker_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, session)?;
    if let Ok(host) = std::env::var("ZELLIJ_SESSION_NAME") {
        crate::config::set_last_host_session(&host, session)?;
    }
    Ok(())
}

/// Clears host Workspace attachment marker.
pub fn clear_workspace_session() -> Result<()> {
    std::env::remove_var(WORKSPACE_SESSION_ENV);
    if let Some(path) = workspace_marker_path() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct HostPaneInfo {
    id: u32,
    is_plugin: bool,
    is_focused: bool,
    pane_x: usize,
    title: String,
}

/// Returns title of pane immediately right of current TUI pane.
/// Used to recover attachment state from hosts created before the marker existed.
pub fn workspace_pane_title() -> Result<Option<String>> {
    let output = Command::new("zellij")
        .args(["action", "list-panes", "--tab", "--json"])
        .output()
        .context("Failed to inspect host panes")?;

    if !output.status.success() {
        return Ok(None);
    }

    let panes: Vec<HostPaneInfo> = serde_json::from_slice(&output.stdout)
        .context("Failed to parse host pane information")?;
    let current_id = std::env::var("ZELLIJ_PANE_ID")
        .ok()
        .and_then(|id| {
            id.rsplit('_')
                .next()
                .and_then(|value| value.parse::<u32>().ok())
        });
    let current = current_id
        .and_then(|id| panes.iter().find(|pane| pane.id == id))
        .or_else(|| panes.iter().find(|pane| pane.is_focused));
    let Some(current) = current else {
        return Ok(None);
    };

    Ok(panes
        .iter()
        .filter(|pane| !pane.is_plugin && pane.pane_x > current.pane_x)
        .min_by_key(|pane| pane.pane_x)
        .map(|pane| pane.title.clone()))
}

/// Renames the currently focused pane in the host session.
///
/// The caller must focus the Workspace pane before invoking this action.
pub fn rename_workspace_pane(name: &str) -> Result<()> {
    let status = Command::new("zellij")
        .args(["action", "rename-pane", name])
        .status()
        .context("Failed to rename workspace pane")?;

    if !status.success() {
        eprintln!(
            "[verij-cli] Warning: rename-pane '{}' exited with status: {}",
            name, status
        );
    }

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
    set_workspace_session(target_session)?;
    let marker_cleanup = workspace_marker_path()
        .map(|path| format!("; rm -f {}", shell_quote(&path.to_string_lossy())))
        .unwrap_or_default();
    let resurrection_flag = matches!(
        crate::session::session_status(target_session),
        Ok(crate::session::SessionStatus::Exited)
    );
    if resurrection_flag {
        let _ = crate::session::prepare_resurrection_layout(target_session);
    }
    let attach_options = if resurrection_flag {
        "--force-run-commands "
    } else {
        ""
    };
    let attach_cmd = format!(
        "export {WORKSPACE_SESSION_ENV}={}; stty sane; zellij attach {attach_options}{}; unset {WORKSPACE_SESSION_ENV}{marker_cleanup}\n",
        shell_quote(target_session),
        shell_quote(target_session),
    );
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
        let _ = clear_workspace_session();
        eprintln!(
            "[verij-cli] Warning: initial attach write-chars exited with status: {}",
            status
        );
    }

    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"))
}

fn wait_for_layout_plugin(session_name: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if session_has_visible_plugin(session_name) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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
