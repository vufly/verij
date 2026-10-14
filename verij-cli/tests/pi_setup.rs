use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use verij_types::agent::{
    ActivityKind, AgentIdentity, AgentKind, AgentState, NormalizedSourceRecord,
    ReducedExecutionState,
};
use verij_types::identity::{
    AgentInstanceId, PaneKey, ProcessIdentity, SessionInstanceId, TerminalPaneId,
};

fn make_fake_pi(path: &Path, version: &str) {
    let script = format!("#!/bin/sh\nprintf '{version}\\n'\n");
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn process_identity_of(pid: u32) -> ProcessIdentity {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let (_, suffix) = stat.rsplit_once(')').unwrap();
    let fields: Vec<&str> = suffix.split_whitespace().collect();
    let start: u64 = fields[19].parse().unwrap();
    let uid = unsafe { libc::getuid() };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_string();

    ProcessIdentity {
        pid,
        start_jiffies: start,
        boot_id,
        uid,
    }
}

struct TestPtyChild {
    child: std::process::Child,
    master_fd: std::os::raw::c_int,
}

impl Drop for TestPtyChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        unsafe {
            libc::close(self.master_fd);
        }
    }
}

fn spawn_pty_child() -> Result<TestPtyChild, Box<dyn std::error::Error>> {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if master < 0 {
        return Err("posix_openpt failed".into());
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

    let slave_in = unsafe { libc::open(slave_name.as_ptr(), libc::O_RDWR) };
    let slave_out = unsafe { libc::open(slave_name.as_ptr(), libc::O_RDWR) };
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

    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30").stdin(in_file).stdout(out_file);
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            let slave = libc::open(
                slave_name_bytes.as_ptr() as *const libc::c_char,
                libc::O_RDWR,
            );
            if slave >= 0 {
                libc::ioctl(slave, libc::TIOCSCTTY, 0);
                libc::tcsetpgrp(slave, libc::getpid());
                libc::close(slave);
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    std::thread::sleep(Duration::from_millis(50));
    Ok(TestPtyChild {
        child,
        master_fd: master,
    })
}

fn cli(root: &Path, args: &[&str], extra_env: &[(&str, &Path)]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_verij"));
    cmd.current_dir(root)
        .args(args)
        .env("HOME", root)
        .env("VERIJ_AGENT_STATE_DIR", root.join("runtime"))
        .env("VERIJ_STATES_DIR", root.join("states"));

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    cmd.output().unwrap()
}

#[test]
fn test_setup_and_uninstall_lifecycle_idempotency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home with 'spaces and quotes'");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];

    // 1. First setup
    let output = cli(&root, &setup_args, &[]);
    assert!(
        output.status.success(),
        "Setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(installed["installed"], true);
    assert_eq!(installed["adapter"], "pi");
    assert_eq!(installed["version"], "1.2.0");

    // Verify file locations: extensions in extensions/verij-monitoring, config in sibling verij-monitoring outside extension discovery
    assert!(ext_dir.join("index.ts").exists());
    assert!(ext_dir.join("core.mjs").exists());
    assert!(cfg_dir.join("adapter.json").exists());
    assert!(cfg_dir.join("installed.json").exists());
    assert!(!ext_dir.join("adapter.json").exists());
    assert!(!ext_dir.join("installed.json").exists());

    let manifest_bytes = fs::read(cfg_dir.join("installed.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["adapter_id"], "verij-monitoring-v1");
    assert_eq!(manifest["version"], "1.2.0");
    let installed_at_ms = manifest["installed_at_ms"].as_u64().unwrap();

    // 2. Doctor check
    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc_output = cli(&root, &doctor_args, &[]);
    assert!(doc_output.status.success());
    let doc: Value = serde_json::from_slice(&doc_output.stdout).unwrap();
    assert_eq!(doc["adapter"], "pi");
    assert_eq!(doc["version"], "1.2.0");
    assert_eq!(doc["version_supported"], true);
    assert_eq!(doc["assets_intact"], true);
    assert_eq!(doc["index_intact"], true);
    assert_eq!(doc["core_intact"], true);
    assert_eq!(doc["adapter_intact"], true);
    assert_eq!(doc["manifest_valid"], true);
    assert_eq!(doc["reporter_available"], true);
    assert_eq!(doc["runtime_writable"], true);
    assert_eq!(doc["discovery_disabled"], false);
    assert_eq!(doc["structural_installation_ready"], true);
    assert_eq!(doc["installation_ready"], true);
    assert_eq!(doc["adapter_loaded"], false);
    assert_eq!(doc["live_records"], 0);

    // 3. Second setup (byte-idempotent, installed_at_ms preserved)
    let output2 = cli(&root, &setup_args, &[]);
    assert!(output2.status.success());
    let manifest2_bytes = fs::read(cfg_dir.join("installed.json")).unwrap();
    let manifest2: Value = serde_json::from_slice(&manifest2_bytes).unwrap();
    assert_eq!(
        manifest2["installed_at_ms"].as_u64().unwrap(),
        installed_at_ms
    );
    assert_eq!(manifest_bytes, manifest2_bytes);

    // 4. Uninstall
    let uninstall_args = [
        "agent",
        "uninstall",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let un_output = cli(&root, &uninstall_args, &[]);
    assert!(un_output.status.success());
    let un_val: Value = serde_json::from_slice(&un_output.stdout).unwrap();
    assert_eq!(un_val["uninstalled"], true);

    assert!(!ext_dir.exists());
    assert!(!cfg_dir.exists());

    // 5. Doctor after uninstall reports not ready
    let doc_output2 = cli(&root, &doctor_args, &[]);
    assert!(doc_output2.status.success());
    let doc2: Value = serde_json::from_slice(&doc_output2.stdout).unwrap();
    assert_eq!(doc2["installation_ready"], false);
    assert_eq!(doc2["assets_intact"], false);

    // 6. Repeated uninstall (idempotent)
    let un_output2 = cli(&root, &uninstall_args, &[]);
    assert!(un_output2.status.success());
    let un_val2: Value = serde_json::from_slice(&un_output2.stdout).unwrap();
    assert_eq!(un_val2["uninstalled"], true);
}

#[test]
fn test_unowned_directory_adoption_and_dotfile_preservation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");

    // Case 1: unowned extension directory without manifest -> setup must reject adoption
    fs::create_dir_all(&ext_dir).unwrap();
    fs::write(ext_dir.join("index.ts"), "// user extension").unwrap();

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let output1 = cli(&root, &setup_args, &[]);
    assert!(
        !output1.status.success(),
        "Setup should fail when extension dir exists without ownership manifest"
    );
    let err1 = String::from_utf8_lossy(&output1.stderr);
    assert!(err1.contains("unowned extension directory") || err1.contains("manifest"));

    // Case 2: empty {} manifest -> setup must reject
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(cfg_dir.join("installed.json"), "{}").unwrap();
    let output2 = cli(&root, &setup_args, &[]);
    assert!(!output2.status.success());

    // Case 3: JSON null manifest -> setup must reject
    fs::write(cfg_dir.join("installed.json"), "null").unwrap();
    let output3 = cli(&root, &setup_args, &[]);
    assert!(!output3.status.success());

    // Case 4: Unowned directory must NEVER be deleted by uninstall; uninstall bails
    let uninstall_args = [
        "agent",
        "uninstall",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let un_out = cli(&root, &uninstall_args, &[]);
    assert!(
        !un_out.status.success(),
        "Uninstall without valid manifest must fail"
    );
    assert!(
        ext_dir.join("index.ts").exists(),
        "Unowned user files must remain intact"
    );

    // Clean up unowned dir
    fs::remove_dir_all(&agent_dir).unwrap();

    // Clean setup
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Case 5: Dotfile custom extension: .custom.ts importing ./core.mjs
    // has_extra_files must NOT ignore dotfiles!
    fs::write(ext_dir.join(".custom.ts"), "import './core.mjs';\n").unwrap();

    let un_dot = cli(&root, &uninstall_args, &[]);
    assert!(un_dot.status.success());
    let un_dot_val: Value = serde_json::from_slice(&un_dot.stdout).unwrap();
    // Must report uninstalled: false because code remains and autoloads!
    assert_eq!(un_dot_val["uninstalled"], false);

    // Both index.ts and core.mjs must be preserved to avoid breaking .custom.ts
    assert!(ext_dir.join("index.ts").exists());
    assert!(ext_dir.join("core.mjs").exists());
    assert!(ext_dir.join(".custom.ts").exists());
    assert!(cfg_dir.join("adapter.json").exists());

    // Manifest must NOT be rewritten to uninstalled=true while extension code remains
    let manifest_val: Value =
        serde_json::from_slice(&fs::read(cfg_dir.join("installed.json")).unwrap()).unwrap();
    assert_ne!(manifest_val.get("uninstalled"), Some(&json!(true)));
}

#[test]
fn test_custom_env_paths_and_tilde_and_relative_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    // Test 1: PI_CODING_AGENT_DIR with ~/custom-agent
    let custom_agent = root.join("custom-agent");
    let custom_agent_str = Path::new("~/custom-agent");
    let output = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ],
        &[("PI_CODING_AGENT_DIR", custom_agent_str)],
    );
    assert!(
        output.status.success(),
        "Setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(custom_agent
        .join("extensions/verij-monitoring/index.ts")
        .exists());
    assert!(custom_agent.join("verij-monitoring/adapter.json").exists());

    // Verify adapter.json contains absolute paths
    let adapter_bytes = fs::read(custom_agent.join("verij-monitoring/adapter.json")).unwrap();
    let adapter: Value = serde_json::from_slice(&adapter_bytes).unwrap();
    assert!(Path::new(adapter["reporter"].as_str().unwrap()).is_absolute());
    assert!(Path::new(adapter["runtime_dir"].as_str().unwrap()).is_absolute());
    assert!(Path::new(adapter["states_dir"].as_str().unwrap()).is_absolute());

    // Test 2: explicit --config-dir wins over PI_CODING_AGENT_DIR
    let explicit_dir = root.join("explicit-agent");
    let output2 = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--config-dir",
            explicit_dir.to_str().unwrap(),
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ],
        &[("PI_CODING_AGENT_DIR", custom_agent_str)],
    );
    assert!(output2.status.success());
    assert!(explicit_dir
        .join("extensions/verij-monitoring/index.ts")
        .exists());

    // Test 3: empty PI_CODING_AGENT_DIR="" falls back to default ~/.pi/agent
    let default_agent = root.join(".pi/agent");
    let output3 = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ],
        &[("PI_CODING_AGENT_DIR", Path::new(""))],
    );
    assert!(output3.status.success());
    assert!(default_agent
        .join("extensions/verij-monitoring/index.ts")
        .exists());

    // Test 4: file: URLs rejected with clear error before writes
    let file_url_dir = root.join("file-url-agent");
    let file_url_str = format!("file://{}", file_url_dir.display());
    let output4 = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ],
        &[("PI_CODING_AGENT_DIR", Path::new(&file_url_str))],
    );
    assert!(!output4.status.success(), "file: URLs must be rejected");
    let err = String::from_utf8_lossy(&output4.stderr);
    assert!(
        err.contains("file: URLs are not supported"),
        "Unexpected error: {err}"
    );
    assert!(
        !file_url_dir.exists(),
        "No directory should be created for rejected file: URL"
    );
}

#[test]
fn test_modified_assets_preserved_on_uninstall_and_prevent_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Modify index.ts
    let modified_code = "// user customized index.ts\nimport './core.mjs';\n";
    fs::write(ext_dir.join("index.ts"), modified_code).unwrap();

    // Uninstall: index.ts must be preserved, core.mjs MUST be preserved because index.ts imports it,
    // and adapter.json MUST be preserved because preserved code references it
    let uninstall_args = [
        "agent",
        "uninstall",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let un_output = cli(&root, &uninstall_args, &[]);
    assert!(un_output.status.success());

    assert_eq!(
        fs::read_to_string(ext_dir.join("index.ts")).unwrap(),
        modified_code
    );
    assert!(
        ext_dir.join("core.mjs").exists(),
        "core.mjs must be preserved when index.ts is preserved"
    );
    assert!(
        cfg_dir.join("adapter.json").exists(),
        "adapter.json must NOT be deleted while preserved code still references it"
    );

    // Setup now must REFUSE to overwrite modified asset without preservation
    let setup_clobber = cli(&root, &setup_args, &[]);
    assert!(
        !setup_clobber.status.success(),
        "Setup should fail when asset is modified"
    );
    let err = String::from_utf8_lossy(&setup_clobber.stderr);
    assert!(
        err.contains("modified adapter asset"),
        "Unexpected error: {err}"
    );

    // Doctor reports assets not intact
    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc = cli(&root, &doctor_args, &[]);
    let doc_val: Value = serde_json::from_slice(&doc.stdout).unwrap();
    assert_eq!(doc_val["assets_intact"], false);
    assert_eq!(doc_val["installation_ready"], false);
}

#[test]
fn test_invalid_utf8_preserved_and_prevents_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Write invalid UTF-8 bytes to core.mjs
    let invalid_bytes = vec![0xFF, 0xFE, 0x00, 0xBA, 0xDD];
    fs::write(ext_dir.join("core.mjs"), &invalid_bytes).unwrap();

    // Uninstall: core.mjs must be preserved, and entire code/config group must be preserved
    let uninstall_args = [
        "agent",
        "uninstall",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let un_output = cli(&root, &uninstall_args, &[]);
    assert!(un_output.status.success());

    assert_eq!(fs::read(ext_dir.join("core.mjs")).unwrap(), invalid_bytes);
    assert!(cfg_dir.join("adapter.json").exists());

    // Setup must fail to overwrite invalid UTF-8 asset
    let setup_clobber = cli(&root, &setup_args, &[]);
    assert!(!setup_clobber.status.success());

    // Also test invalid UTF-8 in adapter.json directly
    fs::remove_file(ext_dir.join("core.mjs")).unwrap();
    fs::write(cfg_dir.join("adapter.json"), &invalid_bytes).unwrap();

    let un_output2 = cli(&root, &uninstall_args, &[]);
    assert!(un_output2.status.success());
    assert_eq!(
        fs::read(cfg_dir.join("adapter.json")).unwrap(),
        invalid_bytes
    );

    let setup_clobber2 = cli(&root, &setup_args, &[]);
    assert!(!setup_clobber2.status.success());
}

#[test]
fn test_disabled_discovery_and_reinclude_and_project_uncertainty() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    fs::create_dir_all(&agent_dir).unwrap();

    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Case A: exclusion "-extensions/verij-monitoring" in settings.json
    let settings_a = json!({
        "extensions": ["-extensions/verij-monitoring"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_a).unwrap(),
    )
    .unwrap();
    let doc_a = cli(&root, &doctor_args, &[]);
    let doc_a_val: Value = serde_json::from_slice(&doc_a.stdout).unwrap();
    assert_eq!(doc_a_val["discovery_disabled"], true);
    assert_eq!(
        doc_a_val["discovery_exclusion"],
        "-extensions/verij-monitoring"
    );
    assert_eq!(doc_a_val["installation_ready"], false);

    // Case B: exclusion "!**/verij-monitoring"
    let settings_b = json!({
        "extensions": ["!**/verij-monitoring"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_b).unwrap(),
    )
    .unwrap();
    let doc_b = cli(&root, &doctor_args, &[]);
    let doc_b_val: Value = serde_json::from_slice(&doc_b.stdout).unwrap();
    assert_eq!(doc_b_val["discovery_disabled"], true);
    assert_eq!(doc_b_val["installation_ready"], false);

    // Case C: exclusion "!extensions/*"
    let settings_c = json!({
        "extensions": ["!extensions/*"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_c).unwrap(),
    )
    .unwrap();
    let doc_c = cli(&root, &doctor_args, &[]);
    let doc_c_val: Value = serde_json::from_slice(&doc_c.stdout).unwrap();
    assert_eq!(doc_c_val["discovery_disabled"], true);
    assert_eq!(doc_c_val["installation_ready"], false);

    // Case D: non-matching prefix "-extensions/verij-monitoring-other" must NOT disable discovery
    let settings_d = json!({
        "extensions": ["-extensions/verij-monitoring-other"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_d).unwrap(),
    )
    .unwrap();
    let doc_d = cli(&root, &doctor_args, &[]);
    let doc_d_val: Value = serde_json::from_slice(&doc_d.stdout).unwrap();
    assert_eq!(doc_d_val["discovery_disabled"], false);
    assert_eq!(doc_d_val["installation_ready"], true);

    // Case E: Force include with + re-enables what ! glob excluded
    let settings_e = json!({
        "extensions": ["!**/verij-monitoring", "+extensions/verij-monitoring"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_e).unwrap(),
    )
    .unwrap();
    let doc_e = cli(&root, &doctor_args, &[]);
    let doc_e_val: Value = serde_json::from_slice(&doc_e.stdout).unwrap();
    assert_eq!(doc_e_val["discovery_disabled"], false);
    assert_eq!(doc_e_val["installation_ready"], true);

    // Case E2: Exact minus exclusion overrides plus regardless of order (minus evaluated after plus)
    let settings_e2 = json!({
        "extensions": ["+extensions/verij-monitoring", "-extensions/verij-monitoring"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_e2).unwrap(),
    )
    .unwrap();
    let doc_e2 = cli(&root, &doctor_args, &[]);
    let doc_e2_val: Value = serde_json::from_slice(&doc_e2.stdout).unwrap();
    assert_eq!(doc_e2_val["discovery_disabled"], true);
    assert_eq!(doc_e2_val["installation_ready"], false);

    // Case E3: Minus with glob is treated literally as string, does NOT disable
    let settings_e3 = json!({
        "extensions": ["-extensions/*"]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_e3).unwrap(),
    )
    .unwrap();
    let doc_e3 = cli(&root, &doctor_args, &[]);
    let doc_e3_val: Value = serde_json::from_slice(&doc_e3.stdout).unwrap();
    assert_eq!(doc_e3_val["discovery_disabled"], false);
    assert_eq!(doc_e3_val["installation_ready"], true);

    // Case E4: Absolute minus exclusion disables discovery
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let settings_e4 = json!({
        "extensions": [format!("-{}", ext_dir.display())]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_e4).unwrap(),
    )
    .unwrap();
    let doc_e4 = cli(&root, &doctor_args, &[]);
    let doc_e4_val: Value = serde_json::from_slice(&doc_e4.stdout).unwrap();
    assert_eq!(doc_e4_val["discovery_disabled"], true);
    assert_eq!(doc_e4_val["installation_ready"], false);

    // Case E5: Absolute plus inclusion re-enables what ! glob excluded
    let settings_e5 = json!({
        "extensions": ["!**/verij-monitoring", format!("+{}", ext_dir.display())]
    });
    fs::write(
        agent_dir.join("settings.json"),
        serde_json::to_vec(&settings_e5).unwrap(),
    )
    .unwrap();
    let doc_e5 = cli(&root, &doctor_args, &[]);
    let doc_e5_val: Value = serde_json::from_slice(&doc_e5.stdout).unwrap();
    assert_eq!(doc_e5_val["discovery_disabled"], false);
    assert_eq!(doc_e5_val["installation_ready"], true);

    // Clean settings
    fs::write(agent_dir.join("settings.json"), "{}").unwrap();

    // Case F: Project settings in .pi/settings.json treated as uncertainty, NOT blocker
    let proj_pi = root.join(".pi");
    fs::create_dir_all(&proj_pi).unwrap();
    let proj_settings = json!({
        "extensions": ["-extensions/verij-monitoring"]
    });
    fs::write(
        proj_pi.join("settings.json"),
        serde_json::to_vec(&proj_settings).unwrap(),
    )
    .unwrap();
    let doc_f = cli(&root, &doctor_args, &[]);
    let doc_f_val: Value = serde_json::from_slice(&doc_f.stdout).unwrap();
    assert_eq!(doc_f_val["discovery_disabled"], false);
    assert_eq!(doc_f_val["installation_ready"], true);
    let uncertainties = doc_f_val["discovery_uncertainties"].as_array().unwrap();
    assert!(uncertainties
        .iter()
        .any(|u| u.as_str().unwrap().contains("Project settings")));
}

#[test]
fn test_symlink_safety_refuses_operations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    fs::create_dir_all(agent_dir.join("extensions")).unwrap();

    // 1. Symlink for extensions/verij-monitoring pointing outside
    let foreign_dir = temp.path().join("foreign_extensions");
    fs::create_dir_all(&foreign_dir).unwrap();
    std::os::unix::fs::symlink(&foreign_dir, agent_dir.join("extensions/verij-monitoring"))
        .unwrap();

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let setup_out = cli(&root, &setup_args, &[]);
    assert!(
        !setup_out.status.success(),
        "Setup should fail when target is a symlink"
    );
    let err = String::from_utf8_lossy(&setup_out.stderr);
    assert!(err.contains("symlink"), "Unexpected error: {err}");

    // 2. Ancestor extensions directory is a symlink
    fs::remove_file(agent_dir.join("extensions/verij-monitoring")).unwrap();
    fs::remove_dir(agent_dir.join("extensions")).unwrap();
    std::os::unix::fs::symlink(&foreign_dir, agent_dir.join("extensions")).unwrap();

    let setup_out2 = cli(&root, &setup_args, &[]);
    assert!(
        !setup_out2.status.success(),
        "Setup should fail when ancestor extensions directory is a symlink"
    );
    let err2 = String::from_utf8_lossy(&setup_out2.stderr);
    assert!(err2.contains("symlink"), "Unexpected error: {err2}");
}

#[test]
fn test_probe_version_deadline_and_process_group_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let script_path = temp.path().join("hanging_pi");

    // Script closes stdout immediately (sending EOF) and sleeps for 8 seconds
    let script = "#!/bin/sh\nexec 1>&-\nsleep 8\n";
    fs::write(&script_path, script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    let start = Instant::now();
    let res = verij_cli_pi_setup_probe_version(&script_path);
    let elapsed = start.elapsed();

    assert!(res.is_err(), "Hanging script must fail with timeout");
    let err = res.err().unwrap().to_string();
    assert!(err.contains("timed out"), "Expected timeout, got: {err}");
    // Must complete near 3 seconds (not hang for 8s)
    assert!(
        elapsed < Duration::from_millis(4200),
        "Probe took too long: {:?}",
        elapsed
    );
}

fn verij_cli_pi_setup_probe_version(path: &Path) -> anyhow::Result<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_verij"))
        .args(["agent", "doctor", "pi", "--pi-bin", path.to_str().unwrap()])
        .output()?;
    let val: Value = serde_json::from_slice(&output.stdout)?;
    if val["binary_available"].as_bool() == Some(true) {
        Ok(val["version"].as_str().unwrap().to_string())
    } else {
        anyhow::bail!(
            "{}",
            val["blockers"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("timed out")
        )
    }
}

#[test]
fn test_unsupported_versions_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");

    for version in ["1.2.1", "1.3.0", "2.0.0", "0.9.0", "1.2.0-beta.1"] {
        make_fake_pi(&fake_pi, version);

        let setup_args = [
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ];
        let setup_out = cli(&root, &setup_args, &[]);
        assert!(
            !setup_out.status.success(),
            "Version {version} should fail setup"
        );
        let err = String::from_utf8_lossy(&setup_out.stderr);
        assert!(
            err.contains("unsupported Pi version"),
            "Unexpected error: {err}"
        );

        let doctor_args = [
            "agent",
            "doctor",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ];
        let doc_out = cli(&root, &doctor_args, &[]);
        assert!(doc_out.status.success());
        let doc: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
        assert_eq!(doc["version"], version);
        assert_eq!(doc["version_supported"], false);
        assert_eq!(doc["supported_versions"], json!(["1.2.0"]));
        assert_eq!(doc["installation_ready"], false);
    }
}

#[test]
fn test_missing_reporter_and_verij_bin_override() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Clobber reporter in adapter.json to nonexistent binary
    let adapter_path = root.join(".pi/agent/verij-monitoring/adapter.json");
    let mut adapter: Value = serde_json::from_slice(&fs::read(&adapter_path).unwrap()).unwrap();
    adapter["reporter"] = json!("/nonexistent/bin/verij");
    fs::write(&adapter_path, serde_json::to_vec_pretty(&adapter).unwrap()).unwrap();

    // 1. Doctor without override reports reporter unavailable
    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc_out = cli(&root, &doctor_args, &[]);
    assert!(doc_out.status.success());
    let doc: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
    assert_eq!(doc["reporter_available"], false);
    assert_eq!(doc["installation_ready"], false);

    // 2. Doctor with VERIJ_BIN override pointing to valid binary succeeds
    let valid_verij = Path::new(env!("CARGO_BIN_EXE_verij"));
    let doc_override = cli(&root, &doctor_args, &[("VERIJ_BIN", valid_verij)]);
    assert!(doc_override.status.success());
    let doc_val2: Value = serde_json::from_slice(&doc_override.stdout).unwrap();
    assert_eq!(doc_val2["reporter_available"], true);
    assert_eq!(doc_val2["reporter"], valid_verij.to_str().unwrap());
}

#[test]
fn test_inventory_diagnostics_missing_and_legacy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    let states_dir = root.join("states");
    fs::create_dir_all(&states_dir).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];

    // 1. Missing inventory directory / empty states
    let doc_out1 = cli(&root, &doctor_args, &[]);
    assert!(doc_out1.status.success());
    let doc1: Value = serde_json::from_slice(&doc_out1.stdout).unwrap();
    assert_eq!(doc1["inventory_ready"], false);

    // 2. Legacy snapshot without inventory
    let legacy_snapshot = json!({
        "name": "session-legacy",
        "is_current": true,
        "tabs": [],
        "active_pane": null
    });
    fs::write(
        states_dir.join("session-legacy.json"),
        serde_json::to_vec(&legacy_snapshot).unwrap(),
    )
    .unwrap();

    let doc_out2 = cli(&root, &doctor_args, &[]);
    let doc2: Value = serde_json::from_slice(&doc_out2.stdout).unwrap();
    assert_eq!(doc2["inventory_sessions"][0]["status"], "legacy_snapshot");
    assert_eq!(doc2["inventory_ready"], false);

    // 3. Ready snapshot with verified pane
    let server_proc = process_identity_of(std::process::id());
    let ready_snapshot = json!({
        "name": "session-ready",
        "is_current": true,
        "tabs": [],
        "active_pane": null,
        "inventory": {
            "schema_version": 1,
            "producer": {"server_pid": std::process::id(), "plugin_id": 0, "client_id": 1, "epoch": "test"},
            "revision": 1,
            "exported_at_ms": u64::MAX,
            "tabs": [],
            "panes": [{
                "terminal_id": 1,
                "tab_position": 0,
                "title": "pi",
                "pane_pid": std::process::id(),
                "pane_process": server_proc,
                "is_floating": false,
                "is_suppressed": false,
                "is_fullscreen": false,
                "layer_focused": false,
                "exited": false
            }],
            "capabilities": [],
            "server_process": server_proc
        }
    });
    fs::write(
        states_dir.join("session-ready.json"),
        serde_json::to_vec(&ready_snapshot).unwrap(),
    )
    .unwrap();

    let doc_out3 = cli(&root, &doctor_args, &[]);
    let doc3: Value = serde_json::from_slice(&doc_out3.stdout).unwrap();
    assert_eq!(doc3["inventory_ready"], true);
}

#[test]
fn test_doctor_distinguishes_structural_readiness_from_loaded_adapter() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    let states_dir = root.join("states");
    let runtime_dir = root.join("runtime");
    fs::create_dir_all(&states_dir).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let pty_child = spawn_pty_child().expect("spawn pty child");
    let child_pid = pty_child.child.id();
    let server_proc = process_identity_of(std::process::id());
    let child_proc = process_identity_of(child_pid);
    let session_instance_id = SessionInstanceId::from_process(&server_proc);

    let ready_snapshot = json!({
        "name": "test-session",
        "is_current": true,
        "tabs": [],
        "active_pane": null,
        "inventory": {
            "schema_version": 1,
            "producer": {"server_pid": std::process::id(), "plugin_id": 0, "client_id": 1, "epoch": "test"},
            "revision": 1,
            "exported_at_ms": u64::MAX,
            "tabs": [],
            "panes": [{
                "terminal_id": 1,
                "tab_position": 0,
                "title": "pi",
                "pane_pid": child_pid,
                "pane_process": child_proc,
                "is_floating": false,
                "is_suppressed": false,
                "is_fullscreen": false,
                "layer_focused": false,
                "exited": false
            }],
            "capabilities": [],
            "server_process": server_proc
        }
    });
    fs::write(
        states_dir.join("test-session.json"),
        serde_json::to_vec(&ready_snapshot).unwrap(),
    )
    .unwrap();

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // 1. Without live Pi: structural is true, but adapter_loaded is false
    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc_out = cli(&root, &doctor_args, &[]);
    assert!(doc_out.status.success());
    let doc: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
    assert_eq!(doc["structural_installation_ready"], true);
    assert_eq!(doc["installation_ready"], true);
    assert_eq!(doc["adapter_loaded"], false);
    assert_eq!(doc["live_records"], 0);

    // 2. Simulate a live Pi record running as foreground owner of the PTY pane
    let live_dir = runtime_dir.join("agents/v1/pi-live-instance");
    fs::create_dir_all(&live_dir).unwrap();

    let identity = AgentIdentity {
        schema_version: 1,
        agent_instance_id: AgentInstanceId("pi-live-instance".into()),
        pane_key: PaneKey {
            session: session_instance_id.clone(),
            terminal: TerminalPaneId(1),
        },
        kind: AgentKind::Pi,
        process: child_proc,
        registered_at_ms: 1000,
        is_synthetic: false,
        runner_id: None,
    };
    fs::write(
        live_dir.join("identity.json"),
        serde_json::to_vec(&identity).unwrap(),
    )
    .unwrap();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let mut sources = BTreeMap::new();
    sources.insert(
        "pi".to_string(),
        NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: AgentInstanceId("pi-live-instance".into()),
            source_id: "pi".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: now_ms,
            conversation_id: None,
            conversation_title: Some("test conv".into()),
            turn_id: None,
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![
                "activity".into(),
                "lease".into(),
                "aggregate_completion".into(),
            ],
        },
    );

    let mut adapter_state = BTreeMap::new();
    adapter_state.insert(
        "pi".to_string(),
        json!({
            "running_version": "1.2.0",
            "runtime_id": "runtime-123",
            "conversation_id": "conv-456",
            "source_epoch": 1,
            "source_revision": 1
        }),
    );

    let state = AgentState {
        schema_version: 1,
        agent_instance_id: AgentInstanceId("pi-live-instance".into()),
        record_revision: 1,
        updated_at_ms: now_ms,
        sources,
        reduced: ReducedExecutionState {
            status: verij_types::agent::AgentStatus::Working,
            current_turn_id: None,
            current_turn_epoch: 1,
            current_turn_revision: 1,
            conversation_id: None,
            conversation_title: Some("test conv".into()),
            pending_requests: vec![],
            background_task_count: 0,
            latest_error: None,
            latest_completion: None,
            completion_revision: 1,
            detail: Some("working".into()),
        },
        completion_revision: 1,
        completed_turn: None,
        completed_turn_epoch: 1,
        completed_turn_revision: 1,
        opencode_completions: BTreeMap::new(),
        adapter_state,
    };
    fs::write(
        live_dir.join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();

    let doc_out2 = cli(&root, &doctor_args, &[]);
    assert!(doc_out2.status.success());
    let doc2: Value = serde_json::from_slice(&doc_out2.stdout).unwrap();
    assert_eq!(doc2["structural_installation_ready"], true);
    assert_eq!(doc2["adapter_loaded"], true);
    assert_eq!(doc2["live_records"], 1);
    assert_eq!(doc2["records"][0]["running_version"], "1.2.0");
    assert_eq!(doc2["records"][0]["source_healthy"], true);
    assert_eq!(doc2["records"][0]["is_foreground_owner"], true);
    assert_eq!(doc2["monitoring_prerequisites_ready"], true);

    // Test --session filter: matching session name
    let doc_match = cli(
        &root,
        &[
            "agent",
            "doctor",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
            "--session",
            "test-session",
        ],
        &[],
    );
    let doc_match_val: Value = serde_json::from_slice(&doc_match.stdout).unwrap();
    assert_eq!(doc_match_val["live_records"], 1);
    assert_eq!(doc_match_val["adapter_loaded"], true);

    // Test --session filter: non-matching session name excludes records
    let doc_mismatch = cli(
        &root,
        &[
            "agent",
            "doctor",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
            "--session",
            "other-session",
        ],
        &[],
    );
    let doc_mismatch_val: Value = serde_json::from_slice(&doc_mismatch.stdout).unwrap();
    assert_eq!(doc_mismatch_val["live_records"], 0);
    assert_eq!(doc_mismatch_val["adapter_loaded"], false);
}

#[test]
fn test_doctor_manifest_integrity_error_handled_gracefully() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Corrupt installed.json with invalid JSON
    let manifest_path = root.join(".pi/agent/verij-monitoring/installed.json");
    fs::write(&manifest_path, "not json").unwrap();

    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc_out = cli(&root, &doctor_args, &[]);
    assert!(
        doc_out.status.success(),
        "Doctor must not crash on invalid manifest"
    );
    let doc: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
    assert_eq!(doc["manifest_valid"], false);
    assert_eq!(doc["installation_ready"], false);
    let blockers = doc["blockers"].as_array().unwrap();
    assert!(blockers
        .iter()
        .any(|b| b.as_str().unwrap().contains("manifest")));
}

#[test]
fn test_file_url_rejected_with_clear_error() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let file_url = "file:///tmp/agent%20dir";

    // 1. --config-dir
    let setup_out = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
            "--config-dir",
            file_url,
        ],
        &[],
    );
    assert!(
        !setup_out.status.success(),
        "file: URL --config-dir must fail"
    );
    let err = String::from_utf8_lossy(&setup_out.stderr);
    assert!(
        err.contains("file: URLs are not supported"),
        "Unexpected error: {err}"
    );

    // 2. PI_CODING_AGENT_DIR
    let setup_env_out = cli(
        &root,
        &[
            "agent",
            "setup",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
        ],
        &[("PI_CODING_AGENT_DIR", Path::new(file_url))],
    );
    assert!(
        !setup_env_out.status.success(),
        "file: URL in PI_CODING_AGENT_DIR must fail"
    );
    let err_env = String::from_utf8_lossy(&setup_env_out.stderr);
    assert!(
        err_env.contains("file: URLs are not supported"),
        "Unexpected error: {err_env}"
    );

    // 3. Doctor also rejects file: URL
    let doc_out = cli(
        &root,
        &[
            "agent",
            "doctor",
            "pi",
            "--pi-bin",
            fake_pi.to_str().unwrap(),
            "--config-dir",
            file_url,
        ],
        &[],
    );
    assert!(!doc_out.status.success(), "file: URL in doctor must fail");
}

#[test]
fn test_cfg_extra_dotfile_preserves_whole_group() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("pi");
    make_fake_pi(&fake_pi, "1.2.0");

    let agent_dir = root.join(".pi/agent");
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, &[]).status.success());

    // Record initial file contents
    let init_index = fs::read(ext_dir.join("index.ts")).unwrap();
    let init_core = fs::read(ext_dir.join("core.mjs")).unwrap();
    let init_adapter = fs::read(cfg_dir.join("adapter.json")).unwrap();
    let init_manifest = fs::read(cfg_dir.join("installed.json")).unwrap();

    // Add extra dotfile in cfg_dir
    let extra_cfg = cfg_dir.join(".custom_cfg");
    fs::write(&extra_cfg, b"user custom config").unwrap();

    // Uninstall: must preserve the entire group and report uninstalled: false
    let uninstall_args = [
        "agent",
        "uninstall",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let un_out = cli(&root, &uninstall_args, &[]);
    assert!(un_out.status.success());
    let un_val: Value = serde_json::from_slice(&un_out.stdout).unwrap();
    assert_eq!(un_val["uninstalled"], false);

    let preserved = un_val["preserved"].as_array().unwrap();
    assert!(preserved
        .iter()
        .any(|p| p.as_str().unwrap().contains(".custom_cfg")));

    // All original files and bytes must be completely intact and unchanged
    assert_eq!(fs::read(ext_dir.join("index.ts")).unwrap(), init_index);
    assert_eq!(fs::read(ext_dir.join("core.mjs")).unwrap(), init_core);
    assert_eq!(
        fs::read(cfg_dir.join("adapter.json")).unwrap(),
        init_adapter
    );
    assert_eq!(
        fs::read(cfg_dir.join("installed.json")).unwrap(),
        init_manifest
    );
    assert_eq!(fs::read(&extra_cfg).unwrap(), b"user custom config");
}

#[test]
fn test_probe_large_output_drained_completely() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_pi = root.join("large_output_pi");
    // Generate > 512 bytes output: 1.2.0\n followed by 1000 'a' characters
    let padding = "a".repeat(1000);
    let script = format!("#!/bin/sh\nprintf '1.2.0\\n{padding}\\n'\n");
    fs::write(&fake_pi, script).unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let setup_args = [
        "agent",
        "setup",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let setup_out = cli(&root, &setup_args, &[]);
    assert!(
        setup_out.status.success(),
        "Large output probe should extract version successfully"
    );

    let doctor_args = [
        "agent",
        "doctor",
        "pi",
        "--pi-bin",
        fake_pi.to_str().unwrap(),
    ];
    let doc_out = cli(&root, &doctor_args, &[]);
    assert!(doc_out.status.success());
    let doc: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
    assert_eq!(doc["version"], "1.2.0");
    assert_eq!(doc["version_supported"], true);
}
