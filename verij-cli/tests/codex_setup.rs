use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn make_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn cli(root: &Path, args: &[&str], codex_home: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_verij"));
    cmd.args(args)
        .env("HOME", root)
        .env("VERIJ_AGENT_STATE_DIR", root.join("runtime"))
        .env("VERIJ_STATES_DIR", root.join("states"));

    if let Some(ch) = codex_home {
        cmd.env("CODEX_HOME", ch);
    } else {
        cmd.env_remove("CODEX_HOME");
    }

    cmd.output().unwrap()
}

fn cli_with_runtime(root: &Path, args: &[&str], runtime: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_verij"));
    cmd.args(args)
        .env("HOME", root)
        .env("VERIJ_STATES_DIR", root.join("states"))
        .env_remove("CODEX_HOME");
    if let Some(rt) = runtime {
        cmd.env("VERIJ_AGENT_STATE_DIR", rt);
    } else {
        cmd.env_remove("VERIJ_AGENT_STATE_DIR");
    }
    cmd.output().unwrap()
}

fn cli_no_runtime_env(root: &Path, args: &[&str]) -> std::process::Output {
    cli_with_runtime(root, args, None)
}

#[test]
fn test_fresh_setup_preserves_user_settings_and_is_byte_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home with 'spaces and quotes'");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let hooks_file = codex_dir.join("hooks.json");
    let user_hooks = json!({
        "metadata": {"user_config_version": 42},
        "hooks": {
            "Stop": [
                {
                    "matcher": "user-matcher",
                    "hooks": [
                        {"type": "command", "command": "echo user-stop", "timeout": 5}
                    ]
                }
            ]
        }
    });
    fs::write(&hooks_file, serde_json::to_vec_pretty(&user_hooks).unwrap()).unwrap();

    let config_toml = codex_dir.join("config.toml");
    fs::write(&config_toml, "[model]\ndefault = \"gpt-5\"\n").unwrap();

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    let output = cli(&root, &setup_args, None);
    assert!(
        output.status.success(),
        "Setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let assets = codex_dir.join("verij-monitoring-codex");
    assert!(assets.join("callback.py").exists());
    assert!(assets.join("adapter.json").exists());
    assert!(assets.join("installed.json").exists());

    let adapter: Value =
        serde_json::from_slice(&fs::read(assets.join("adapter.json")).unwrap()).unwrap();
    assert_eq!(adapter["enabled"], true);
    assert_eq!(adapter["version"], "0.162.1");

    let installed_hooks_bytes = fs::read(&hooks_file).unwrap();
    let hooks_val: Value = serde_json::from_slice(&installed_hooks_bytes).unwrap();

    assert_eq!(hooks_val["metadata"]["user_config_version"], 42);
    assert_eq!(
        fs::read_to_string(&config_toml).unwrap(),
        "[model]\ndefault = \"gpt-5\"\n"
    );

    let hooks_map = hooks_val["hooks"].as_object().unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "Stop",
        "Interrupt",
        "SessionEnd",
    ] {
        assert!(hooks_map.contains_key(event), "Missing event {event}");
    }

    let stop_groups = hooks_map["Stop"].as_array().unwrap();
    assert_eq!(stop_groups.len(), 2);
    assert_eq!(stop_groups[0]["matcher"], "user-matcher");
    assert_eq!(stop_groups[0]["hooks"][0]["command"], "echo user-stop");
    assert_eq!(stop_groups[1]["hooks"][0]["timeout"], 2);

    // Byte idempotence
    let output2 = cli(&root, &setup_args, None);
    assert!(output2.status.success());
    let second_hooks_bytes = fs::read(&hooks_file).unwrap();
    assert_eq!(installed_hooks_bytes, second_hooks_bytes);
}

#[test]
fn test_doctor_diagnostics_and_prerequisites() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home 'quote'");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    let doctor_args = [
        "agent",
        "doctor",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    let output = cli(&root, &doctor_args, None);
    assert!(
        output.status.success(),
        "Doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["adapter"], "codex");
    assert_eq!(doc["version"], "0.162.1");
    assert_eq!(doc["supported_version"], true);
    assert_eq!(doc["binary_available"], true);
    assert_eq!(doc["binary_matches"], true);
    assert_eq!(doc["callback_present"], true);
    assert_eq!(doc["callback_intact"], true);
    assert_eq!(doc["adapter_intact"], true);
    assert_eq!(doc["hooks_installed"], true);
    assert_eq!(doc["reporter_available"], true);
    assert_eq!(doc["runtime_writable"], true);
    assert_eq!(doc["installation_ready"], true);
    assert_eq!(doc["inventory_ready"], false);
    assert_eq!(doc["monitoring_prerequisites_ready"], false);
    assert_eq!(doc["active_instances"], 0);
    assert_eq!(doc["interactive_mode"], "--no-daemon");
    assert_eq!(doc["hook_trust_required"], true);
    assert_eq!(doc["native_terminal_errors_supported"], false);
}

#[test]
fn test_doctor_remains_ready_after_minor_upgrade_and_binary_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_upgrade");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    assert!(cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap()
        ],
        None
    )
    .status
    .success());

    // Replace fake codex with fresh file (new inode)
    let replacement = root.join("codex-new");
    make_executable(&replacement, "#!/bin/sh\nprintf 'codex-cli 0.200.2\\n'\n");
    fs::rename(&replacement, &fake_codex).unwrap();

    let out = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["binary_matches"], false);
    assert_eq!(doc["version"], "0.200.2");
    assert_eq!(doc["installation_ready"], true);
    assert!(doc["installation_error"].is_null());
}

#[test]
fn test_doctor_missing_manifest_and_lookalike_command() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_doc_manifest");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    assert!(cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap()
        ],
        None
    )
    .status
    .success());

    // Remove manifest
    let manifest_path = codex_dir.join("verij-monitoring-codex/installed.json");
    fs::remove_file(&manifest_path).unwrap();

    let out = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["installation_ready"], false);
    assert!(doc["installation_error"]
        .as_str()
        .unwrap()
        .contains("manifest"));
}

#[test]
fn test_unowned_adapter_and_invalid_manifest_all_bytes_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_unowned");
    let codex_dir = root.join(".codex");
    let assets = codex_dir.join("verij-monitoring-codex");
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    // Unowned adapter.json without manifest
    let unowned_adapter = assets.join("adapter.json");
    let adapter_content = "{\"user_custom\": true}";
    fs::write(&unowned_adapter, adapter_content).unwrap();

    let out = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unowned adapter assets"));
    assert_eq!(
        fs::read_to_string(&unowned_adapter).unwrap(),
        adapter_content
    );

    // Now test with invalid/malformed manifest
    let manifest_path = assets.join("installed.json");
    fs::write(&manifest_path, "{broken json").unwrap();
    let out2 = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(!out2.status.success());
    assert_eq!(
        fs::read_to_string(&unowned_adapter).unwrap(),
        adapter_content
    );
    assert_eq!(fs::read_to_string(&manifest_path).unwrap(), "{broken json");
}

#[test]
fn test_edited_owned_matcher_setup_fails_doctor_false_uninstall_retains() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_matcher");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // User adds matcher to Verij's installed group for SessionStart
    let hooks_file = codex_dir.join("hooks.json");
    let mut hooks_val: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    hooks_val["hooks"]["SessionStart"][0]["matcher"] = json!("user-added-matcher");
    fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks_val).unwrap()).unwrap();

    // 1. Re-run setup must fail to protect user edit
    let out_setup = cli(&root, &setup_args, None);
    assert!(!out_setup.status.success());
    assert!(String::from_utf8_lossy(&out_setup.stderr).contains("group metadata/matcher"));

    // 2. Doctor reports hooks_installed: false
    let out_doc = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out_doc.status.success());
    let doc: Value = serde_json::from_slice(&out_doc.stdout).unwrap();
    assert_eq!(doc["hooks_installed"], false);
    assert_eq!(doc["installation_ready"], false);

    // 3. Uninstall retains group, handler, callback, and adapter (disabled)
    let out_uninst = cli(&root, &["agent", "uninstall", "codex"], None);
    assert!(out_uninst.status.success());

    let after_hooks: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    assert_eq!(
        after_hooks["hooks"]["SessionStart"][0]["matcher"],
        "user-added-matcher"
    );

    let assets = codex_dir.join("verij-monitoring-codex");
    assert!(assets.join("callback.py").exists());
    assert!(assets.join("adapter.json").exists());
    let adapter: Value =
        serde_json::from_slice(&fs::read(assets.join("adapter.json")).unwrap()).unwrap();
    assert_eq!(adapter["enabled"], false);
}

#[test]
fn test_user_empty_group_and_unrelated_handler_inside_owned_group_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_preserve_groups");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let hooks_file = codex_dir.join("hooks.json");
    // User already has an empty group with matcher
    let initial_hooks = json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "user-empty-matcher",
                    "hooks": []
                }
            ]
        }
    });
    fs::write(
        &hooks_file,
        serde_json::to_vec_pretty(&initial_hooks).unwrap(),
    )
    .unwrap();

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // Inside Verij's owned group, user adds a second handler
    let mut hooks_val: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    // Index 1 is Verij's group
    hooks_val["hooks"]["SessionStart"][1]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "command", "command": "echo user-companion", "timeout": 1}));
    fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks_val).unwrap()).unwrap();

    // Uninstall
    assert!(cli(&root, &["agent", "uninstall", "codex"], None)
        .status
        .success());

    let after_hooks: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    let groups = after_hooks["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    // User empty group preserved!
    assert_eq!(groups[0]["matcher"], "user-empty-matcher");
    assert_eq!(groups[0]["hooks"].as_array().unwrap().len(), 0);
    // User companion handler preserved in group 1!
    assert_eq!(groups[1]["hooks"].as_array().unwrap().len(), 1);
    assert_eq!(groups[1]["hooks"][0]["command"], "echo user-companion");
}

#[test]
fn test_references_from_extra_event_retains_assets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_extra_event");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // Add extra event referencing callback.py
    let hooks_file = codex_dir.join("hooks.json");
    let mut hooks_val: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    let callback_cmd = hooks_val["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    hooks_val["hooks"]["CustomExtraEvent"] = json!([
        {
            "hooks": [
                {"type": "command", "command": callback_cmd, "timeout": 2}
            ]
        }
    ]);
    fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks_val).unwrap()).unwrap();

    // Uninstall
    assert!(cli(&root, &["agent", "uninstall", "codex"], None)
        .status
        .success());

    // Assets must be retained because CustomExtraEvent references them
    let assets = codex_dir.join("verij-monitoring-codex");
    assert!(assets.join("callback.py").exists());
    assert!(assets.join("adapter.json").exists());
    assert!(assets.join("installed.json").exists());
    let adapter: Value =
        serde_json::from_slice(&fs::read(assets.join("adapter.json")).unwrap()).unwrap();
    assert_eq!(adapter["enabled"], false);
}

#[test]
fn test_tampered_adapter_fails_setup_with_all_other_assets_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_tamper_adapter");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    let assets = codex_dir.join("verij-monitoring-codex");
    let adapter_path = assets.join("adapter.json");
    let callback_path = assets.join("callback.py");
    let hooks_path = codex_dir.join("hooks.json");

    let initial_callback = fs::read(&callback_path).unwrap();
    let initial_hooks = fs::read(&hooks_path).unwrap();

    // Tamper adapter.json
    fs::write(&adapter_path, "{\"tampered\": true}").unwrap();

    let out = cli(&root, &setup_args, None);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("modified Codex adapter configuration"));

    // Other files must remain completely unchanged
    assert_eq!(fs::read(&callback_path).unwrap(), initial_callback);
    assert_eq!(fs::read(&hooks_path).unwrap(), initial_hooks);
}

#[test]
fn test_malformed_hooks_json_fails_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_malformed");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    // Malformed JSON (hooks field has group missing hooks array)
    let hooks_file = codex_dir.join("hooks.json");
    let malformed_content = "{\"hooks\": {\"Stop\": [{\"missing_hooks_key\": true}]}}";
    fs::write(&hooks_file, malformed_content).unwrap();

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    let out = cli(&root, &setup_args, None);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing 'hooks' array"));

    // Content untouched
    assert_eq!(fs::read_to_string(&hooks_file).unwrap(), malformed_content);
    assert!(!codex_dir.join("verij-monitoring-codex").exists());
}

#[test]
fn test_malformed_hooks_uninstall_leaves_all_files_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_uninst_malformed");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    assert!(cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap()
        ],
        None
    )
    .status
    .success());

    let assets = codex_dir.join("verij-monitoring-codex");
    let callback_bytes = fs::read(assets.join("callback.py")).unwrap();
    let adapter_bytes = fs::read(assets.join("adapter.json")).unwrap();
    let manifest_bytes = fs::read(assets.join("installed.json")).unwrap();

    // Corrupt hooks.json
    let hooks_file = codex_dir.join("hooks.json");
    let corrupted = "{corrupted json";
    fs::write(&hooks_file, corrupted).unwrap();

    let out = cli(&root, &["agent", "uninstall", "codex"], None);
    assert!(!out.status.success());

    // All files must be byte-identical
    assert_eq!(fs::read_to_string(&hooks_file).unwrap(), corrupted);
    assert_eq!(
        fs::read(assets.join("callback.py")).unwrap(),
        callback_bytes
    );
    assert_eq!(
        fs::read(assets.join("adapter.json")).unwrap(),
        adapter_bytes
    );
    assert_eq!(
        fs::read(assets.join("installed.json")).unwrap(),
        manifest_bytes
    );
}

#[test]
fn test_config_dir_and_codex_home_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_precedence");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let dir_default = root.join(".codex");
    let dir_env = root.join("env_codex");
    let dir_cli = root.join("cli_codex");

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let out1 = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        Some(&dir_env),
    );
    assert!(out1.status.success());
    assert!(dir_env.join("verij-monitoring-codex").exists());
    assert!(!dir_default.exists());

    let out2 = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--config-dir",
            dir_cli.to_str().unwrap(),
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        Some(&dir_env),
    );
    assert!(out2.status.success());
    assert!(dir_cli.join("verij-monitoring-codex").exists());
}

#[test]
fn test_unsupported_and_missing_binary_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_unsupported");
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let out = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            "/nonexistent/codex",
        ],
        None,
    );
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["binary_available"], false);
    assert_eq!(doc["supported_version"], false);
    assert_eq!(doc["installation_ready"], false);

    let out_setup = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            "/nonexistent/codex",
        ],
        None,
    );
    assert!(!out_setup.status.success());

    let bad_codex = root.join("bad_codex");
    make_executable(&bad_codex, "#!/bin/sh\nprintf 'codex-cli 0.99.0\\n'\n");
    let out_bad = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            bad_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(!out_bad.status.success());
    assert!(String::from_utf8_lossy(&out_bad.stderr).contains("unsupported Codex version"));

    let wrong_prefix = root.join("wrong_prefix");
    make_executable(
        &wrong_prefix,
        "#!/bin/sh\nprintf 'openai-codex 0.162.1\\n'\n",
    );
    let out_prefix = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            wrong_prefix.to_str().unwrap(),
        ],
        None,
    );
    assert!(!out_prefix.status.success());
    assert!(String::from_utf8_lossy(&out_prefix.stderr).contains("invalid Codex version prefix"));
}

#[test]
fn test_reinstall_different_python_path_updates_hooks_and_manifest_and_doctor_ready() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_reinstall_python");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let py1 = root.join("py1");
    make_executable(&py1, "#!/bin/sh\nexit 0\n");
    let py2 = root.join("py2");
    make_executable(&py2, "#!/bin/sh\nexit 0\n");

    // 1. Initial setup with py1
    let out1 = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
            "--python-bin",
            py1.to_str().unwrap(),
        ],
        None,
    );
    assert!(
        out1.status.success(),
        "Initial setup failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );

    let assets = codex_dir.join("verij-monitoring-codex");
    let manifest1: Value =
        serde_json::from_slice(&fs::read(assets.join("installed.json")).unwrap()).unwrap();
    assert_eq!(manifest1["python_bin"], py1.to_str().unwrap());

    let hooks_file = codex_dir.join("hooks.json");
    let hooks1: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    let cmd1 = hooks1["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(cmd1.contains(py1.to_str().unwrap()));

    // 2. Re-run setup with py2
    let out2 = cli(
        &root,
        &[
            "agent",
            "setup",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
            "--python-bin",
            py2.to_str().unwrap(),
        ],
        None,
    );
    assert!(
        out2.status.success(),
        "Re-setup with py2 failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    let manifest2: Value =
        serde_json::from_slice(&fs::read(assets.join("installed.json")).unwrap()).unwrap();
    assert_eq!(manifest2["python_bin"], py2.to_str().unwrap());

    let hooks2: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    let cmd2 = hooks2["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(cmd2.contains(py2.to_str().unwrap()));
    assert!(!cmd2.contains(py1.to_str().unwrap()));

    // 3. Doctor passes with py2
    let out_doc = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out_doc.status.success());
    let doc: Value = serde_json::from_slice(&out_doc.stdout).unwrap();
    assert_eq!(doc["python_available"], true);
    assert_eq!(doc["hooks_installed"], true);
    assert_eq!(doc["installation_ready"], true);
}

#[test]
fn test_extra_owned_handler_field_fails_setup_doctor_false_uninstall_preserves() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_extra_handler_field");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // User edits owned handler to add additionalContext
    let hooks_file = codex_dir.join("hooks.json");
    let mut hooks_val: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    hooks_val["hooks"]["SessionStart"][0]["hooks"][0]["additionalContext"] = json!(true);
    let tampered_hooks_bytes = serde_json::to_vec_pretty(&hooks_val).unwrap();
    fs::write(&hooks_file, &tampered_hooks_bytes).unwrap();

    // 1. Re-run setup must fail and preserve all bytes
    let out_setup = cli(&root, &setup_args, None);
    assert!(!out_setup.status.success());
    assert!(String::from_utf8_lossy(&out_setup.stderr).contains("edited Verij hook handler"));
    assert_eq!(fs::read(&hooks_file).unwrap(), tampered_hooks_bytes);

    // 2. Doctor reports hooks_installed: false
    let out_doc = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out_doc.status.success());
    let doc: Value = serde_json::from_slice(&out_doc.stdout).unwrap();
    assert_eq!(doc["hooks_installed"], false);
    assert_eq!(doc["installation_ready"], false);

    // 3. Uninstall retains the edited handler and assets
    let out_uninst = cli(&root, &["agent", "uninstall", "codex"], None);
    assert!(out_uninst.status.success());

    let after_hooks: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    assert_eq!(
        after_hooks["hooks"]["SessionStart"][0]["hooks"][0]["additionalContext"],
        true
    );

    let assets = codex_dir.join("verij-monitoring-codex");
    assert!(assets.join("callback.py").exists());
    assert!(assets.join("adapter.json").exists());
    assert!(assets.join("installed.json").exists());
    let adapter: Value =
        serde_json::from_slice(&fs::read(assets.join("adapter.json")).unwrap()).unwrap();
    assert_eq!(adapter["enabled"], false);
}

#[test]
fn test_exact_command_collision_without_manifest_fails_setup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_exact_collision");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let assets = codex_dir.join("verij-monitoring-codex");
    let callback_path = assets.join("callback.py");

    // Write hooks.json containing exact callback command before setup
    let hooks_file = codex_dir.join("hooks.json");
    let collision_cmd = format!("python3 {} SessionStart", callback_path.display());
    let hooks_val = json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [
                        {"type": "command", "command": collision_cmd, "timeout": 2}
                    ]
                }
            ]
        }
    });
    fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks_val).unwrap()).unwrap();

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    let out = cli(&root, &setup_args, None);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ownership collision without valid manifest")
    );

    // No manifest or adapter written
    assert!(!assets.join("installed.json").exists());
    assert!(!assets.join("adapter.json").exists());
}

#[test]
fn test_extra_event_with_spaces_and_quotes_preserves_on_uninstall() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home 'with spaces and quotes'");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // Copy exact owned command into a custom event
    let hooks_file = codex_dir.join("hooks.json");
    let mut hooks_val: Value = serde_json::from_slice(&fs::read(&hooks_file).unwrap()).unwrap();
    let owned_cmd = hooks_val["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();

    hooks_val["hooks"]["CustomExtraEvent"] = json!([
        {
            "hooks": [
                {"type": "command", "command": owned_cmd, "timeout": 2}
            ]
        }
    ]);
    fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks_val).unwrap()).unwrap();

    // Uninstall
    let out_uninst = cli(&root, &["agent", "uninstall", "codex"], None);
    assert!(out_uninst.status.success());

    // Assets retained because CustomExtraEvent references callback
    let assets = codex_dir.join("verij-monitoring-codex");
    assert!(assets.join("callback.py").exists());
    assert!(assets.join("adapter.json").exists());
    assert!(assets.join("installed.json").exists());
    let adapter: Value =
        serde_json::from_slice(&fs::read(assets.join("adapter.json")).unwrap()).unwrap();
    assert_eq!(adapter["enabled"], false);
}

#[test]
fn test_invalid_utf8_callback_uninstall_preserves() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_invalid_utf8");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // Replace callback.py with invalid UTF-8 bytes
    let assets = codex_dir.join("verij-monitoring-codex");
    let callback_file = assets.join("callback.py");
    let invalid_bytes = vec![0xFF, 0xFE, 0xFD];
    fs::write(&callback_file, &invalid_bytes).unwrap();

    // Uninstall
    let out_uninst = cli(&root, &["agent", "uninstall", "codex"], None);
    assert!(out_uninst.status.success());

    // callback.py preserved
    assert!(callback_file.exists());
    assert_eq!(fs::read(&callback_file).unwrap(), invalid_bytes);
}

#[test]
fn test_doctor_missing_python_executable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_missing_python");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    let fake_py = root.join("fake_python");
    make_executable(&fake_py, "#!/bin/sh\nexit 0\n");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
        "--python-bin",
        fake_py.to_str().unwrap(),
    ];
    assert!(cli(&root, &setup_args, None).status.success());

    // Remove python executable
    fs::remove_file(&fake_py).unwrap();

    // Doctor reports python_available: false, installation_ready: false
    let out_doc = cli(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
        None,
    );
    assert!(out_doc.status.success());
    let doc: Value = serde_json::from_slice(&out_doc.stdout).unwrap();
    assert_eq!(doc["python_available"], false);
    assert_eq!(doc["installation_ready"], false);
    assert!(doc["installation_error"]
        .as_str()
        .unwrap()
        .contains("Python"));
}

#[test]
fn test_doctor_adapter_runtime_unwritable_file_blocking_without_env() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("home_blocked_runtime");
    let codex_dir = root.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(root.join("states")).unwrap();

    let fake_codex = root.join("codex");
    make_executable(&fake_codex, "#!/bin/sh\nprintf 'codex-cli 0.162.1\\n'\n");

    // Blocked path configured via VERIJ_AGENT_STATE_DIR during setup
    let blocked_dir = root.join("blocked_dir");
    let blocked_sub = blocked_dir.join("sub_runtime");

    let setup_args = [
        "agent",
        "setup",
        "codex",
        "--codex-bin",
        fake_codex.to_str().unwrap(),
    ];
    assert!(cli_with_runtime(&root, &setup_args, Some(&blocked_sub))
        .status
        .success());

    // Remove directory and create a regular file at blocked_dir so sub_runtime can never be created
    let _ = fs::remove_dir_all(&blocked_dir);
    fs::write(&blocked_dir, "regular file blocks dir").unwrap();

    // Run doctor without VERIJ_AGENT_STATE_DIR env var
    let out_doc = cli_no_runtime_env(
        &root,
        &[
            "agent",
            "doctor",
            "codex",
            "--codex-bin",
            fake_codex.to_str().unwrap(),
        ],
    );
    assert!(out_doc.status.success());
    let doc: Value = serde_json::from_slice(&out_doc.stdout).unwrap();
    assert_eq!(doc["runtime_writable"], false);
    assert_eq!(doc["monitoring_prerequisites_ready"], false);
}
