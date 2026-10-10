//! Existing Magy watch records are observed without reading request/prompt data.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use verij_types::agent::*;
use verij_types::identity::{PaneKey, ProcessIdentity, SessionInstanceId, TerminalPaneId};

pub fn state_directory() -> Result<PathBuf> {
    let path = std::env::var_os("VERIJ_MAGY_STATE_DIR")
        .or_else(|| std::env::var_os("MAGY_STATE_DIR"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|p| PathBuf::from(p).join("magy")))
        .or_else(|| {
            std::env::var_os("MAGY_REAL_HOME")
                .or_else(|| std::env::var_os("HOME"))
                .map(|p| PathBuf::from(p).join(".local/state/magy"))
        })
        .context("Magy state directory unavailable")?;
    if !path.is_absolute() {
        bail!("Magy state directory must be absolute");
    }
    Ok(path)
}

#[derive(Debug, Deserialize)]
struct WatchState {
    watch_id: String,
    status: String,
    session: Option<String>,
    pane_id: Option<String>,
    runner_pid: Option<u32>,
    runner_create_time: Option<f64>,
    conversation_id: Option<String>,
    pty_log_path: Option<PathBuf>,
}

fn valid_watch(id: &str) -> bool {
    id.strip_prefix("watch_")
        .is_some_and(|suffix| suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn read_state(directory: &Path) -> Result<WatchState> {
    let mut bytes = Vec::new();
    std::fs::File::open(directory.join("state.json"))?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        bail!("oversized Magy state");
    }
    let state: WatchState = serde_json::from_slice(&bytes)?;
    if !valid_watch(&state.watch_id)
        || directory.file_name().and_then(|p| p.to_str()) != Some(&state.watch_id)
        || !matches!(
            state.status.as_str(),
            "running" | "completed" | "failed" | "cancelled"
        )
    {
        bail!("invalid Magy watch state");
    }
    Ok(state)
}

fn runner_binding(
    directory: &Path,
    state: &WatchState,
    server: &ProcessIdentity,
    pane: &ProcessIdentity,
) -> Result<ProcessIdentity> {
    let runner = crate::process::identity(state.runner_pid.context("watch has no runner")?)?;
    let expected = state
        .runner_create_time
        .context("watch runner birth unavailable")?;
    let actual = crate::process::started_at_ms(&runner)? as f64 / 1000.0;
    if !expected.is_finite() || (expected - actual).abs() > 0.02 {
        bail!("Magy runner lifetime changed");
    }
    // The trusted renderer owns pane stdio; redirected Agy children do not.
    crate::process::verify_foreground(server, pane, &runner)?;
    let command = std::fs::read(format!("/proc/{}/cmdline", runner.pid))?;
    if command.len() > 16384 {
        bail!("oversized runner identity");
    }
    let args: Vec<_> = command
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .collect();
    let canonical = directory.canonicalize()?;
    if !args.windows(3).any(|w| {
        w[0] == b"-m"
            && w[1] == b"magy.watch_runner"
            && std::str::from_utf8(w[2]).ok().is_some_and(|path| {
                Path::new(path).canonicalize().ok().as_ref() == Some(&canonical)
            })
    }) {
        bail!("process is not this watch's renderer");
    }
    // Python's setenv may not update its initial /proc environ range. The
    // direct Magy child receives both markers at exec, proving runner binding
    // independently from the launcher's inherited Zellij pane environment.
    let children =
        std::fs::read_to_string(format!("/proc/{}/task/{}/children", runner.pid, runner.pid))?;
    let watch_marker = format!("MAGY_WATCH_ID={}", state.watch_id);
    let run_marker = format!("MAGY_RUN_ID={}", state.watch_id);
    let marked_child = children
        .split_whitespace()
        .take(32)
        .filter_map(|p| p.parse::<u32>().ok())
        .any(|pid| {
            std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environment| {
                environment.len() <= 262144
                    && environment
                        .split(|b| *b == 0)
                        .any(|value| value == watch_marker.as_bytes())
                    && environment
                        .split(|b| *b == 0)
                        .any(|value| value == run_marker.as_bytes())
            })
        });
    if !marked_child {
        bail!("watch renderer's marked child is unavailable");
    }
    Ok(runner)
}

/// Revalidate runner identity for navigation, rendering and acknowledgement.
pub fn verify_owner(
    identity: &AgentIdentity,
    server: &ProcessIdentity,
    pane: &ProcessIdentity,
) -> Result<()> {
    if let Some(id) = identity.runner_id.as_ref() {
        if identity.kind != AgentKind::Agy || !valid_watch(id) {
            bail!("invalid monitored runner");
        }
        let directory = state_directory()?.join("watches").join(id);
        let state = read_state(&directory)?;
        if state
            .pane_id
            .as_deref()
            .and_then(|p| p.strip_prefix("terminal_"))
            .and_then(|p| p.parse::<u32>().ok())
            != Some(identity.pane_key.terminal.0)
            || SessionInstanceId::from_process(server) != identity.pane_key.session
            || runner_binding(&directory, &state, server, pane)? != identity.process
        {
            bail!("watch pane/runner binding changed");
        }
        Ok(())
    } else {
        if identity.kind == AgentKind::Agy && !identity.is_synthetic {
            crate::process::verify_agy_foreground(server, pane, &identity.process)
        } else {
            crate::process::verify_foreground(server, pane, &identity.process)
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    device: u64,
    inode: u64,
    conversation: Option<String>,
    result: Option<String>,
    saw_activity: bool,
    #[serde(default)]
    watch_status: String,
    #[serde(default)]
    skipping_line: bool,
}

impl Cursor {
    fn consume(&mut self, event: &serde_json::Value) {
        let kind = event["event"].as_str().unwrap_or("");
        let conversation = match kind {
            "init" => event["conversation_id"].as_str(),
            "step_update" => event["step_update"]["conversation_id"].as_str(),
            "result" => event["result"]["conversation_id"].as_str(),
            _ => return,
        }
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control));
        let Some(conversation) = conversation else {
            return;
        };
        if self
            .conversation
            .as_deref()
            .is_some_and(|id| id != conversation)
        {
            return;
        }
        if self.conversation.is_none() && kind != "init" {
            return;
        }
        self.conversation = Some(conversation.into());
        match kind {
            "init" | "step_update" => self.saw_activity = true,
            "result" => {
                if let Some(status @ ("SUCCESS" | "ERROR")) = event["result"]["status"].as_str() {
                    self.result = Some(status.into());
                }
            }
            _ => (),
        }
    }

    fn tail(&mut self, path: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let mut file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        if (self.device, self.inode) != (metadata.dev(), metadata.ino())
            || metadata.len() < self.offset
        {
            *self = Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                ..Default::default()
            };
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        // Bound allocation and time under record lock; remaining data is read
        // next poll. Huge transcript/output frames are skipped, never stored.
        file.take(1024 * 1024).read_to_end(&mut bytes)?;
        let start = if self.skipping_line {
            match bytes.iter().position(|b| *b == b'\n') {
                Some(n) => {
                    self.skipping_line = false;
                    n + 1
                }
                None => {
                    self.offset += bytes.len() as u64;
                    return Ok(());
                }
            }
        } else {
            0
        };
        let complete = bytes[start..]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(start, |n| n + start + 1);
        for line in bytes[start..complete].split(|b| *b == b'\n') {
            if line.len() <= 262144 {
                if let Ok(value) = serde_json::from_slice(line) {
                    self.consume(&value);
                }
            }
        }
        self.offset += complete as u64;
        if complete == start && bytes.len() == 1024 * 1024 {
            // Advance across oversized lines without interpreting fragments.
            // The next newline is skipped before parsing resumes.
            self.offset += (bytes.len() - complete) as u64;
            self.skipping_line = true;
        }
        Ok(())
    }

    fn project(&self, state: &WatchState, revision: u64) -> PartialSourceRecord {
        let turn_id = Some(TurnId(state.watch_id.clone()));
        let success = state.status == "completed" && self.result.as_deref() == Some("SUCCESS");
        let failed = state.status == "failed"
            || (state.status == "running" && self.result.as_deref() == Some("ERROR"));
        let activity = if matches!(state.status.as_str(), "cancelled" | "completed")
            || self.result.is_some()
        {
            ActivityKind::Idle
        } else if self.saw_activity {
            ActivityKind::Working
        } else {
            ActivityKind::Initializing
        };
        PartialSourceRecord {
            source_id: "magy-watch".into(),
            source_epoch: Some(1),
            source_revision: Some(revision),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: turn_id.clone(),
            conversation_id: self
                .conversation
                .clone()
                .or_else(|| state.conversation_id.clone())
                .map(ConversationId),
            activity: Some(activity),
            background_task_count: Some(0),
            pending_requests: Some(vec![]),
            successful_completion: success.then(|| SuccessfulCompletion {
                turn_id: turn_id.clone(),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: crate::agent_store::now_ms(),
                summary: None,
            }),
            execution_error: failed.then(|| ExecutionError {
                turn_id,
                turn_epoch: 1,
                turn_revision: 1,
                message: "Magy watch failed".into(),
                is_terminal: true,
            }),
            capabilities: Some(vec!["activity".into(), "watch_outcome".into()]),
            ..Default::default()
        }
    }
}

#[derive(Default, Serialize)]
pub struct Diagnostics {
    observed: usize,
    rejected: Vec<serde_json::Value>,
}

pub fn reconcile() -> Result<Diagnostics> {
    let mut diagnostics = Diagnostics::default();
    let root = state_directory()?.join("watches");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(diagnostics);
    };
    let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
    for entry in entries.flatten().take(4096) {
        if !entry.file_name().to_str().is_some_and(valid_watch) {
            continue;
        }
        let directory = entry.path();
        let result = (|| -> Result<()> {
            let state = read_state(&directory)?;
            let terminal = state
                .pane_id
                .as_deref()
                .and_then(|p| p.strip_prefix("terminal_"))
                .and_then(|p| p.parse::<u32>().ok())
                .context("watch has no native terminal")?;
            let process =
                crate::process::identity(state.runner_pid.context("watch has no runner")?)?;
            let mut binding_error = None;
            let owners: Vec<_> = snapshots
                .iter()
                .filter_map(|snapshot| {
                    let inventory = snapshot.inventory.as_ref()?;
                    let server = inventory.server_process.as_ref()?;
                    let pane = inventory
                        .panes
                        .iter()
                        .find(|p| p.terminal_id.0 == terminal && !p.exited)?
                        .pane_process
                        .as_ref()?;
                    let runner = match runner_binding(&directory, &state, server, pane) {
                        Ok(runner) => runner,
                        Err(error) => {
                            binding_error = Some(error.to_string());
                            return None;
                        }
                    };
                    Some((server, runner))
                })
                .collect();
            if owners.len() != 1 || owners[0].1 != process {
                bail!(
                    "watch native pane ownership unavailable or ambiguous: {}",
                    binding_error.unwrap_or_else(|| "native inventory has no matching pane".into())
                );
            }
            // session is a candidate locator; verified ancestry survives rename.
            let _session_hint = &state.session;
            let instance = crate::agent_store::register(
                PaneKey {
                    session: SessionInstanceId::from_process(owners[0].0),
                    terminal: TerminalPaneId(terminal),
                },
                AgentKind::Agy,
                process,
                false,
                Some(state.watch_id.clone()),
            )?;
            let log = directory.join("pty.log");
            if state.pty_log_path.as_ref().is_some_and(|p| p != &log) {
                bail!("watch log binding differs from owned directory");
            }
            crate::agent_store::update(
                &instance,
                |stored| {
                    let mut cursor: Cursor = stored
                        .adapter_state
                        .get("magy-watch")
                        .map(|value| serde_json::from_value(value.clone()))
                        .transpose()?
                        .unwrap_or_default();
                    cursor.tail(&log)?;
                    cursor.watch_status = state.status.clone();
                    if state
                        .conversation_id
                        .as_ref()
                        .is_some_and(|id| cursor.conversation.as_ref().is_some_and(|c| c != id))
                    {
                        bail!("watch conversation differs from bound stream");
                    }
                    let revision = stored
                        .sources
                        .get("magy-watch")
                        .map_or(1, |s| s.source_revision + 1);
                    let partial = cursor.project(&state, revision);
                    if stored.adapter_state.get("magy-watch")
                        == Some(&serde_json::to_value(&cursor)?)
                    {
                        bail!("watch observation unchanged");
                    }
                    // Watch state supersedes stream result; cancellation cannot
                    // retain a previous error or completion through partial merge.
                    stored.sources.remove("magy-watch");
                    stored
                        .adapter_state
                        .insert("magy-watch".into(), serde_json::to_value(cursor)?);
                    Ok(partial)
                },
                |_| Ok(()),
            )?;
            Ok(())
        })();
        // An incomplete/closed/custom-mux record is not a launcher binding.
        match result {
            Ok(()) => diagnostics.observed += 1,
            Err(error) if error.to_string() == "watch observation unchanged" => {
                diagnostics.observed += 1
            }
            Err(error) => {
                if diagnostics.rejected.len() < 64 {
                    diagnostics.rejected.push(serde_json::json!({"watch_id":entry.file_name().to_str(),"reason":error.to_string()}));
                }
            }
        }
    }
    Ok(diagnostics)
}

pub fn run(once: bool) -> Result<()> {
    loop {
        let diagnostics = reconcile()?;
        if once {
            println!("{}", serde_json::to_string(&diagnostics)?);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn watch(status: &str) -> WatchState {
        serde_json::from_value(json!({"watch_id":"watch_0123456789abcdef","status":status}))
            .unwrap()
    }

    #[test]
    fn tool_error_is_not_run_failure_and_stream_success_waits_for_watch_finalization() {
        let mut cursor = Cursor::default();
        cursor.consume(&json!({"event":"init","conversation_id":"c"}));
        cursor.consume(&json!({"event":"step_update","step_update":{"conversation_id":"c","state":"ERROR","step_type":"tool"}}));
        let partial = cursor.project(&watch("running"), 1);
        assert_eq!(partial.activity, Some(ActivityKind::Working));
        assert!(partial.execution_error.is_none());
        cursor.consume(
            &json!({"event":"result","result":{"conversation_id":"foreign","status":"SUCCESS"}}),
        );
        assert!(cursor.result.is_none());
        cursor.consume(
            &json!({"event":"result","result":{"conversation_id":"c","status":"SUCCESS"}}),
        );
        assert!(cursor
            .project(&watch("running"), 2)
            .successful_completion
            .is_none());
        assert!(cursor
            .project(&watch("failed"), 3)
            .execution_error
            .is_some());
        assert!(cursor
            .project(&watch("cancelled"), 4)
            .successful_completion
            .is_none());
        assert!(cursor
            .project(&watch("completed"), 5)
            .successful_completion
            .is_some());
    }

    #[test]
    fn saved_offset_partial_lines_truncation_rotation_and_large_frames_are_bounded() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pty.log");
        std::fs::write(
            &path,
            "{\"event\":\"init\",\"conversation_id\":\"c\"}\n{\"event\":\"result\"",
        )
        .unwrap();
        let mut cursor = Cursor::default();
        cursor.tail(&path).unwrap();
        assert_eq!(cursor.conversation.as_deref(), Some("c"));
        let offset = cursor.offset;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b",\"result\":{\"conversation_id\":\"c\",\"status\":\"SUCCESS\"}}\n")
            .unwrap();
        cursor.tail(&path).unwrap();
        assert!(cursor.offset > offset);
        assert_eq!(cursor.result.as_deref(), Some("SUCCESS"));
        let serialized = serde_json::to_value(&cursor).unwrap();
        let mut restored: Cursor = serde_json::from_value(serialized).unwrap();
        restored.tail(&path).unwrap();
        assert_eq!(restored.offset, cursor.offset);
        std::fs::write(
            &path,
            "{\"event\":\"init\",\"conversation_id\":\"replacement\"}\n",
        )
        .unwrap();
        restored.tail(&path).unwrap();
        assert!(restored.result.is_none());
        assert_eq!(restored.conversation.as_deref(), Some("replacement"));
        let old = root.path().join("old.log");
        std::fs::rename(&path, old).unwrap();
        std::fs::write(&path, "x".repeat(2 * 1024 * 1024)).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n{\"event\":\"init\",\"conversation_id\":\"new\"}\n")
            .unwrap();
        restored.tail(&path).unwrap();
        restored.tail(&path).unwrap();
        restored.tail(&path).unwrap();
        assert_eq!(restored.conversation.as_deref(), Some("new"));
    }
}
