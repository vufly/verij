//! Isolated CLI integration tests for Pi reporter protocol, fencing, and ownership.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(target_os = "linux")]
struct TestPtyChild {
    child: Child,
    master_fd: i32,
}

#[cfg(target_os = "linux")]
impl Drop for TestPtyChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        unsafe {
            libc::close(self.master_fd);
        }
    }
}

/// Owned process wrapper ensuring prompt kill and reap on drop.
struct OwnedProcess(Option<Child>);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl OwnedProcess {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child exists")
    }

    fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        let mut child = self.0.take().expect("child exists");
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(5);
        loop {
            if let Some(_) = child.try_wait()? {
                return child.wait_with_output();
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                panic!("timed out waiting for child process to exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(target_os = "linux")]
fn read_line_timeout<R: std::os::unix::io::AsRawFd + std::io::Read>(
    reader: &mut BufReader<R>,
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    if !reader.buffer().is_empty() {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err("pipe closed (EOF)".into());
        }
        return Ok(line);
    }
    let raw_fd = reader.get_ref().as_raw_fd();
    let mut poll_fd = libc::pollfd {
        fd: raw_fd,
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    let res = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if res < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if res == 0 {
        return Err("timed out waiting for output from reporter".into());
    }
    let mut line = String::new();
    let bytes = reader.read_line(&mut line)?;
    if bytes == 0 {
        return Err("pipe closed (EOF)".into());
    }
    Ok(line)
}

#[cfg(target_os = "linux")]
fn spawn_pty_foreground_process() -> Result<TestPtyChild, Box<dyn std::error::Error>> {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    if master < 0 {
        return Err("posix_openpt failed".into());
    }
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(master, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
    if unsafe { libc::grantpt(master) } != 0 {
        unsafe {
            libc::close(master);
        }
        return Err("grantpt failed".into());
    }
    if unsafe { libc::unlockpt(master) } != 0 {
        unsafe {
            libc::close(master);
        }
        return Err("unlockpt failed".into());
    }
    let mut buf = [0u8; 128];
    if unsafe { libc::ptsname_r(master, buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 {
        unsafe {
            libc::close(master);
        }
        return Err("ptsname_r failed".into());
    }
    let slave_name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
    let slave_name_bytes = slave_name.to_bytes_with_nul().to_vec();

    let slave_in = unsafe { libc::open(slave_name.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    let slave_out = unsafe { libc::open(slave_name.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if slave_in < 0 || slave_out < 0 {
        unsafe {
            if slave_in >= 0 {
                libc::close(slave_in);
            }
            if slave_out >= 0 {
                libc::close(slave_out);
            }
            libc::close(master);
        }
        return Err("open slave pty failed".into());
    }

    let in_file = unsafe { std::fs::File::from_raw_fd(slave_in) };
    let out_file = unsafe { std::fs::File::from_raw_fd(slave_out) };

    let mut cmd = Command::new("sleep");
    cmd.arg("120").stdin(in_file).stdout(out_file);
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let slave = libc::open(
                slave_name_bytes.as_ptr() as *const libc::c_char,
                libc::O_RDWR,
            );
            if slave < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave, libc::TIOCSCTTY, 0) < 0 {
                libc::close(slave);
                return Err(std::io::Error::last_os_error());
            }
            if libc::tcsetpgrp(slave, libc::getpid()) < 0 {
                libc::close(slave);
                return Err(std::io::Error::last_os_error());
            }
            libc::close(slave);
            Ok(())
        });
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            unsafe {
                libc::close(master);
            }
            return Err(e.into());
        }
    };
    std::thread::sleep(Duration::from_millis(50));

    Ok(TestPtyChild {
        child,
        master_fd: master,
    })
}

#[cfg(target_os = "linux")]
fn write_test_session_inventory(
    states_dir: &Path,
    session_name: &str,
    pane_id: u32,
    server_pid: u32,
    child_pid: u32,
) {
    let uid = unsafe { libc::getuid() };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_string();

    let server_stat = std::fs::read_to_string(format!("/proc/{server_pid}/stat")).unwrap();
    let (_, server_suffix) = server_stat.rsplit_once(')').unwrap();
    let server_fields: Vec<&str> = server_suffix.split_whitespace().collect();
    let server_start: u64 = server_fields[19].parse().unwrap();

    let child_stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat")).unwrap();
    let (_, child_suffix) = child_stat.rsplit_once(')').unwrap();
    let child_fields: Vec<&str> = child_suffix.split_whitespace().collect();
    let child_start: u64 = child_fields[19].parse().unwrap();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let snapshot = serde_json::json!({
        "name": session_name,
        "is_current": true,
        "tabs": [{
            "name": "main-tab",
            "position": 0,
            "is_active": true
        }],
        "inventory": {
            "schema_version": 1,
            "producer": {
                "server_pid": server_pid,
                "plugin_id": 1,
                "client_id": 1,
                "epoch": "epoch-test"
            },
            "revision": 1,
            "exported_at_ms": now_ms,
            "session_instance_id": format!("{session_name}-instance"),
            "server_process": {
                "pid": server_pid,
                "start_jiffies": server_start,
                "boot_id": boot_id.clone(),
                "uid": uid
            },
            "capabilities": [],
            "tabs": [{
                "tab_id": 1,
                "position": 0,
                "floating_visible": false,
                "fullscreen_active": false
            }],
            "panes": [{
                "terminal_id": pane_id,
                "tab_id": 1,
                "tab_position": 0,
                "title": "terminal",
                "is_floating": false,
                "is_suppressed": false,
                "is_fullscreen": false,
                "layer_focused": true,
                "exited": false,
                "pane_pid": child_pid,
                "pane_process": {
                    "pid": child_pid,
                    "start_jiffies": child_start,
                    "boot_id": boot_id,
                    "uid": uid
                },
                "stack_id": null
            }]
        }
    });

    fs::write(
        states_dir.join(format!("{session_name}.json")),
        snapshot.to_string(),
    )
    .unwrap();
}

#[test]
fn test_generic_report_rejects_reserved_pi_source() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let mut proc = OwnedProcess::new(
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args(["agent", "report", "--instance", "agent-fake-instance"])
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
            .env("VERIJ_STATES_DIR", &states_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    let payload = serde_json::json!({
        "schema_version": 1,
        "source_id": "pi",
        "source_epoch": 1,
        "source_revision": 1
    });

    if let Some(mut stdin) = proc.child_mut().stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }

    let output = proc.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "generic report must reject source_id 'pi'"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reserved") || stderr.contains("cannot report"),
        "stderr should mention reservation: {stderr}"
    );
}

#[test]
fn test_pi_reporter_fails_when_pane_ownership_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_verij"))
        .args([
            "agent",
            "pi",
            "--pane",
            "42",
            "--pid",
            &std::process::id().to_string(),
        ])
        .env("HOME", root.path())
        .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
        .env("VERIJ_STATES_DIR", &states_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ownership unavailable"),
        "stderr should mention ownership unavailable: {stderr}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_rejects_ambiguous_sessions_regardless_of_hint() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let child_pid = pty.child.id();
    let server_pid = std::process::id();

    // Create TWO sessions in inventory, BOTH having the exact same PTY process as verified owner
    write_test_session_inventory(&states_dir, "session-a", 1, server_pid, child_pid);
    write_test_session_inventory(&states_dir, "session-b", 1, server_pid, child_pid);

    let output = Command::new(env!("CARGO_BIN_EXE_verij"))
        .args([
            "agent",
            "pi",
            "--session",
            "session-a", // Provided hint must NOT bypass global ambiguity resolution
            "--pane",
            "1",
            "--pid",
            &child_pid.to_string(),
        ])
        .env("HOME", root.path())
        .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
        .env("VERIJ_STATES_DIR", &states_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambiguous: 2 verified sessions match pane 1"),
        "stderr should reject verified ambiguity regardless of hint: {stderr}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_rejects_detached_background_child() {
    use std::os::unix::process::CommandExt;

    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let pty_pid = pty.child.id();
    let server_pid = std::process::id();

    // Actual candidate session in inventory
    write_test_session_inventory(&states_dir, "session-test", 1, server_pid, pty_pid);

    // Spawn a detached background child: setsid, inheriting ZELLIJ_* vars, redirected stdio
    let mut cmd = Command::new("sleep");
    cmd.arg("60")
        .env("ZELLIJ_SESSION_NAME", "session-test")
        .env("ZELLIJ_PANE_ID", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let sleep_child = OwnedProcess::new(cmd.spawn().unwrap());
    let sleep_pid = sleep_child.0.as_ref().unwrap().id();

    let output = Command::new(env!("CARGO_BIN_EXE_verij"))
        .args([
            "agent",
            "pi",
            "--session",
            "session-test",
            "--pane",
            "1",
            "--pid",
            &sleep_pid.to_string(),
        ])
        .env("HOME", root.path())
        .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
        .env("VERIJ_STATES_DIR", &states_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ownership unavailable"),
        "detached background child must be rejected against verified session candidate: {stderr}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_ready_success_ordered_snapshots_and_eof_teardown() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let child_pid = pty.child.id();
    let server_pid = std::process::id();

    write_test_session_inventory(&states_dir, "session-test", 1, server_pid, child_pid);

    let mut proc = OwnedProcess::new(
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args([
                "agent",
                "pi",
                "--session",
                "session-test",
                "--pane",
                "1",
                "--pid",
                &child_pid.to_string(),
            ])
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
            .env("VERIJ_STATES_DIR", &states_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    let mut stdin = proc.child_mut().stdin.take().expect("stdin");
    let stdout = proc.child_mut().stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    // 1. Send strict hello frame
    let hello = serde_json::json!({
        "type": "hello",
        "schema_version": 1,
        "version": "1.2.0",
        "runtime_id": "rt-1",
        "conversation_id": "conv-1"
    });
    writeln!(stdin, "{}", hello).unwrap();
    stdin.flush().unwrap();

    // 2. Read ready frame
    let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
    let ready: serde_json::Value = serde_json::from_str(&line).expect("parse ready");
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["status"], "unknown");
    assert_eq!(ready["source_epoch"], 1);
    assert_eq!(ready["source_revision"], 1);
    assert_eq!(ready["restored"], false);
    let agent_instance_id = ready["agent_instance_id"].as_str().unwrap().to_string();

    // 3. Send Working snapshot
    let working_snap = serde_json::json!({
        "schema_version": 1,
        "version": "1.2.0",
        "conversation_id": "conv-1",
        "run_id": "run-1",
        "activity": "working"
    });
    writeln!(stdin, "{}", working_snap).unwrap();
    stdin.flush().unwrap();

    let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
    let receipt1: serde_json::Value = serde_json::from_str(&line).expect("parse receipt1");
    assert_eq!(receipt1["source_epoch"], 1);
    assert_eq!(receipt1["source_revision"], 2);
    assert_eq!(receipt1["status"], "working");

    // 4. Send Done snapshot
    let done_snap = serde_json::json!({
        "schema_version": 1,
        "version": "1.2.0",
        "conversation_id": "conv-1",
        "run_id": "run-1",
        "activity": "idle",
        "outcome": {
            "kind": "completed",
            "completed_at_ms": 1000
        }
    });
    writeln!(stdin, "{}", done_snap).unwrap();
    stdin.flush().unwrap();

    let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
    let receipt2: serde_json::Value = serde_json::from_str(&line).expect("parse receipt2");
    assert_eq!(receipt2["source_epoch"], 1);
    assert_eq!(receipt2["source_revision"], 3);
    assert_eq!(receipt2["status"], "done");

    // 5. EOF teardown
    drop(stdin);
    let output = proc.wait_with_output().unwrap();
    assert!(output.status.success(), "reporter must exit cleanly on EOF");

    // 6. Inspect persisted state file: active status becomes Unknown, but latest_snapshot and completions stay preserved!
    let state_file = runtime_dir
        .join("agents")
        .join("v1")
        .join(&agent_instance_id)
        .join("state.json");
    let state_str = fs::read_to_string(&state_file).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_str).unwrap();
    assert_eq!(state["reduced"]["status"], "unknown");
    assert_eq!(
        state["adapter_state"]["pi"]["latest_snapshot"]["activity"],
        "idle"
    );
    let completions = &state["adapter_state"]["pi"]["completions"];
    let dedup_key = serde_json::to_string(&("conv-1", "run-1")).unwrap();
    assert_eq!(completions[&dedup_key], 1);
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_reconnect_dedup_and_safe_fresh_reload() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let child_pid = pty.child.id();
    let server_pid = std::process::id();

    write_test_session_inventory(&states_dir, "session-test", 1, server_pid, child_pid);

    // Initial run: complete a run with rt-1
    {
        let mut proc = OwnedProcess::new(
            Command::new(env!("CARGO_BIN_EXE_verij"))
                .args([
                    "agent",
                    "pi",
                    "--session",
                    "session-test",
                    "--pane",
                    "1",
                    "--pid",
                    &child_pid.to_string(),
                ])
                .env("HOME", root.path())
                .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
                .env("VERIJ_STATES_DIR", &states_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );

        let mut stdin = proc.child_mut().stdin.take().expect("stdin");
        let stdout = proc.child_mut().stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "type": "hello",
                "schema_version": 1,
                "version": "1.2.0",
                "runtime_id": "rt-1",
                "conversation_id": "conv-1"
            })
        )
        .unwrap();
        stdin.flush().unwrap();
        let _ = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "version": "1.2.0",
                "conversation_id": "conv-1",
                "run_id": "run-1",
                "activity": "idle",
                "outcome": {
                    "kind": "completed",
                    "completed_at_ms": 1000
                }
            })
        )
        .unwrap();
        stdin.flush().unwrap();
        let _ = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();

        drop(stdin);
        assert!(proc.wait_with_output().unwrap().status.success());
    }

    // Reconnect: SAME runtime rt-1 and conversation conv-1
    {
        let mut proc = OwnedProcess::new(
            Command::new(env!("CARGO_BIN_EXE_verij"))
                .args([
                    "agent",
                    "pi",
                    "--session",
                    "session-test",
                    "--pane",
                    "1",
                    "--pid",
                    &child_pid.to_string(),
                ])
                .env("HOME", root.path())
                .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
                .env("VERIJ_STATES_DIR", &states_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );

        let mut stdin = proc.child_mut().stdin.take().expect("stdin");
        let stdout = proc.child_mut().stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "type": "hello",
                "schema_version": 1,
                "version": "1.2.0",
                "runtime_id": "rt-1",
                "conversation_id": "conv-1"
            })
        )
        .unwrap();
        stdin.flush().unwrap();
        let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
        let ready: serde_json::Value = serde_json::from_str(&line).expect("parse ready");
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["restored"], true);
        assert_eq!(ready["status"], "done");
        assert_eq!(ready["aggregate_run_id"], "run-1");
        assert!(ready["latest_snapshot"].is_object());

        drop(stdin);
        assert!(proc.wait_with_output().unwrap().status.success());
    }

    // Fresh reload: DIFFERENT runtime rt-2 (must NOT restore historical completion)
    {
        let mut proc = OwnedProcess::new(
            Command::new(env!("CARGO_BIN_EXE_verij"))
                .args([
                    "agent",
                    "pi",
                    "--session",
                    "session-test",
                    "--pane",
                    "1",
                    "--pid",
                    &child_pid.to_string(),
                ])
                .env("HOME", root.path())
                .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
                .env("VERIJ_STATES_DIR", &states_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );

        let mut stdin = proc.child_mut().stdin.take().expect("stdin");
        let stdout = proc.child_mut().stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "type": "hello",
                "schema_version": 1,
                "version": "1.2.0",
                "runtime_id": "rt-2",
                "conversation_id": "conv-1"
            })
        )
        .unwrap();
        stdin.flush().unwrap();
        let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
        let ready: serde_json::Value = serde_json::from_str(&line).expect("parse ready");
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["restored"], false);
        assert_eq!(ready["status"], "unknown");
        assert_eq!(ready["aggregate_run_id"], serde_json::Value::Null);
        assert_eq!(ready["latest_snapshot"], serde_json::Value::Null);

        drop(stdin);
        assert!(proc.wait_with_output().unwrap().status.success());
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_rejects_duplicate_live_producer() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let child_pid = pty.child.id();
    let server_pid = std::process::id();

    write_test_session_inventory(&states_dir, "session-test", 1, server_pid, child_pid);

    let mut proc1 = OwnedProcess::new(
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args([
                "agent",
                "pi",
                "--session",
                "session-test",
                "--pane",
                "1",
                "--pid",
                &child_pid.to_string(),
            ])
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
            .env("VERIJ_STATES_DIR", &states_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    let mut stdin1 = proc1.child_mut().stdin.take().expect("stdin");
    let stdout1 = proc1.child_mut().stdout.take().expect("stdout");
    let mut reader1 = BufReader::new(stdout1);

    writeln!(
        stdin1,
        "{}",
        serde_json::json!({
            "type": "hello",
            "schema_version": 1,
            "version": "1.2.0",
            "runtime_id": "rt-1",
            "conversation_id": "conv-1"
        })
    )
    .unwrap();
    stdin1.flush().unwrap();
    let _ = read_line_timeout(&mut reader1, Duration::from_secs(5)).unwrap();

    // Second producer on same pane/pid must fail nonblocking
    let proc2 = OwnedProcess::new(
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args([
                "agent",
                "pi",
                "--session",
                "session-test",
                "--pane",
                "1",
                "--pid",
                &child_pid.to_string(),
            ])
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
            .env("VERIJ_STATES_DIR", &states_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    let output2 = proc2.wait_with_output().unwrap();
    assert!(!output2.status.success());
    let stderr = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr.contains("producer already connected"),
        "stderr should mention producer already connected: {stderr}"
    );

    drop(stdin1);
    let _ = proc1.wait_with_output();
}

#[test]
#[cfg(target_os = "linux")]
fn test_pi_reporter_invalid_frame_and_teardown_unknown() {
    let root = tempfile::tempdir().unwrap();
    let runtime_dir = root.path().join("runtime");
    let states_dir = root.path().join("states");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&states_dir).unwrap();

    let pty = spawn_pty_foreground_process().expect("spawn pty");
    let child_pid = pty.child.id();
    let server_pid = std::process::id();

    write_test_session_inventory(&states_dir, "session-test", 1, server_pid, child_pid);

    let mut proc = OwnedProcess::new(
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args([
                "agent",
                "pi",
                "--session",
                "session-test",
                "--pane",
                "1",
                "--pid",
                &child_pid.to_string(),
            ])
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", &runtime_dir)
            .env("VERIJ_STATES_DIR", &states_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    let mut stdin = proc.child_mut().stdin.take().expect("stdin");
    let stdout = proc.child_mut().stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "type": "hello",
            "schema_version": 1,
            "version": "1.2.0",
            "runtime_id": "rt-1",
            "conversation_id": "conv-1"
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let line = read_line_timeout(&mut reader, Duration::from_secs(5)).unwrap();
    let ready: serde_json::Value = serde_json::from_str(&line).expect("parse ready");
    let agent_instance_id = ready["agent_instance_id"].as_str().unwrap().to_string();

    // Send malformed snapshot frame (unknown fields / syntax)
    writeln!(
        stdin,
        "{{\"schema_version\": 1, \"invalid_unknown_field\": true}}"
    )
    .unwrap();
    stdin.flush().unwrap();

    let output = proc.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "reporter must exit on invalid frame"
    );

    // Persisted state must have been downgraded to Unknown by teardown
    let state_file = runtime_dir
        .join("agents")
        .join("v1")
        .join(&agent_instance_id)
        .join("state.json");
    let state_str = fs::read_to_string(&state_file).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_str).unwrap();
    assert_eq!(state["reduced"]["status"], "unknown");
}
