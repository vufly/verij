/// verij-cli/src/pipe_reader.rs
///
/// Spawns the `zellij pipe --name verij_events` subprocess and reads its
/// stdout as a stream of newline-delimited JSON snapshots.
///
/// Each complete line is parsed as `Vec<SessionSnapshot>` and forwarded
/// to the TUI event loop via an `mpsc::Sender`.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Wire-format types (must stay in sync with verij-plugin/src/main.rs)
// ---------------------------------------------------------------------------

/// A single session as received from the plugin's JSON snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionSnapshot {
    pub name: String,
    pub is_current: bool,
    pub tabs: Vec<TabSnapshot>,
}

/// A single tab within a session snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct TabSnapshot {
    pub name: String,
    pub position: usize,
    pub is_active: bool,
}

// ---------------------------------------------------------------------------
// Pipe reader task
// ---------------------------------------------------------------------------

/// Spawns `zellij pipe --name verij_events` and returns an `mpsc::Receiver`
/// that yields a new `Vec<SessionSnapshot>` on every snapshot message.
///
/// This function is intended to be called once at TUI startup. It spawns a
/// dedicated blocking thread (via `tokio::task::spawn_blocking`) to own the
/// subprocess handle and read lines synchronously, then sends parsed snapshots
/// across a bounded channel.
///
/// # Errors
///
/// Returns an error if the subprocess cannot be spawned (e.g. `zellij` not on
/// `$PATH`, or not running inside a Zellij session).
pub fn spawn_pipe_reader(
    tx: mpsc::Sender<Vec<SessionSnapshot>>,
) -> Result<tokio::task::JoinHandle<()>> {
    // Spawn `zellij pipe --name verij_events`.
    // stdout is piped so we can read it line-by-line.
    // stderr is inherited so errors show in the terminal for debugging.
    let child = Command::new("zellij")
        .args(["pipe", "--name", "verij_events"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn `zellij pipe --name verij_events`. Is Zellij running?")?;

    let stdout = child
        .stdout
        .context("Failed to capture stdout from `zellij pipe`")?;

    // Hand off to a blocking task. `BufReader::read_line` blocks, which is
    // incompatible with tokio's cooperative scheduler — hence spawn_blocking.
    let handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let line = match line {
                Ok(l) if !l.trim().is_empty() => l,
                Ok(_) => continue, // skip blank lines
                Err(e) => {
                    eprintln!("[verij] pipe read error: {e}");
                    break;
                }
            };

            match serde_json::from_str::<Vec<SessionSnapshot>>(&line) {
                Ok(snapshot) => {
                    // If the receiver has been dropped (TUI quit), stop.
                    if tx.blocking_send(snapshot).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    // Log malformed messages but keep reading.
                    eprintln!("[verij] Failed to parse snapshot JSON: {e}\n  line: {line}");
                }
            }
        }

        eprintln!("[verij] Pipe reader task exited.");
    });

    Ok(handle)
}
