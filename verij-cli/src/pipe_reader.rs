/// verij-cli/src/pipe_reader.rs
///
/// Spawns the `zellij pipe --name verij_events` subprocess and reads its
/// stdout as a stream of newline-delimited JSON snapshots.
///
/// Each complete line is parsed as `Vec<SessionSnapshot>` and forwarded
/// to the TUI event loop via an `mpsc::Sender`.
use anyhow::Result;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::sync::mpsc;

// Re-export shared wire-format types and constants
pub use verij_types::{SessionSnapshot, VERIJ_EVENTS_PIPE};

/// Spawns `zellij pipe --name verij_events` and forwards snapshots to `tx`.
///
/// If the pipe drops (e.g. during plugin startup or reload), it automatically
/// reconnects with a brief delay until `tx` is closed.
pub fn spawn_pipe_reader(
    tx: mpsc::Sender<Vec<SessionSnapshot>>,
) -> Result<tokio::task::JoinHandle<()>> {
    let handle = tokio::task::spawn_blocking(move || {
        while !tx.is_closed() {
            let mut cmd = Command::new("zellij");
            if let Ok(session) = std::env::var("ZELLIJ_SESSION_NAME") {
                cmd.args(["-s", &session]);
            }
            cmd.args(["pipe", "--name", VERIJ_EVENTS_PIPE]);

            let mut child = match cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    eprintln!("[verij] Failed to spawn `zellij pipe`: {e}");
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            let stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            let mut _stdin = child.stdin.take();
            if let Some(s) = &mut _stdin {
                use std::io::Write;
                let _ = s.write_all(b"connect\n");
                let _ = s.flush();
            }

            let reader = BufReader::new(stdout);
            let mut read_any = false;

            for line in reader.lines() {
                if tx.is_closed() {
                    break;
                }
                let line = match line {
                    Ok(l) if !l.trim().is_empty() => l,
                    Ok(_) => continue,
                    Err(_) => break,
                };

                match serde_json::from_str::<Vec<SessionSnapshot>>(&line) {
                    Ok(snapshot) => {
                        if !snapshot.is_empty() {
                            read_any = true;
                            if tx.blocking_send(snapshot).is_err() {
                                let _ = child.kill();
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[verij] Failed to parse snapshot JSON: {e}\n  line: {line}");
                    }
                }
            }

            let _ = child.kill();
            let _ = child.wait();

            if !read_any {
                std::thread::sleep(Duration::from_millis(300));
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });

    Ok(handle)
}
