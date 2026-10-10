use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_verij"))
        .args(args)
        .env("HOME", root)
        .env("VERIJ_AGENT_STATE_DIR", root.join("runtime"))
        .env("VERIJ_STATES_DIR", root.join("states"))
        .env("VERIJ_MAGY_STATE_DIR", root.join("magy"))
        .output()
        .unwrap()
}

#[test]
fn setup_uninstall_and_copied_profile_diagnostics_preserve_owned_and_user_settings() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("home with 'quote");
    std::fs::create_dir_all(root.join(".gemini/antigravity-cli")).unwrap();
    std::fs::create_dir_all(root.join(".gemini/config")).unwrap();
    std::fs::create_dir_all(root.join("states")).unwrap();
    let binary = root.join("agy");
    std::fs::write(&binary, "#!/bin/sh\nprintf '1.3.3\\n'\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let status_file = root.join(".gemini/antigravity-cli/settings.json");
    let hooks_file = root.join(".gemini/config/hooks.json");
    let original = json!({"theme":"mine","statusLine":{"type":"command","command":"printf original",
        "enabled":true,"padding":3,"stack_with_default":false}});
    std::fs::write(&status_file, serde_json::to_vec(&original).unwrap()).unwrap();
    std::fs::write(&hooks_file, "{\"user-hooks\":{\"Stop\":[]}}").unwrap();
    let args = [
        "agent",
        "setup",
        "agy",
        "--agy-bin",
        binary.to_str().unwrap(),
    ];
    let output = cli(&root, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed = std::fs::read(&status_file).unwrap();
    let installed_hooks = std::fs::read(&hooks_file).unwrap();
    assert!(cli(&root, &args).status.success());
    assert_eq!(std::fs::read(&status_file).unwrap(), installed);
    assert_eq!(std::fs::read(&hooks_file).unwrap(), installed_hooks);
    let status: Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(status["statusLine"]["padding"], 3);
    assert_eq!(status["statusLine"]["stack_with_default"], false);
    assert!(status["statusLine"]["command"]
        .as_str()
        .unwrap()
        .contains("'\\''"));
    let profile = root.join("profile/.gemini");
    std::fs::create_dir_all(profile.join("antigravity-cli")).unwrap();
    std::fs::create_dir_all(profile.join("config")).unwrap();
    std::fs::write(profile.join("antigravity-cli/settings.json"), installed).unwrap();
    std::fs::write(profile.join("config/hooks.json"), installed_hooks).unwrap();
    let output = cli(
        &root,
        &[
            "agent",
            "doctor",
            "agy",
            "--config-dir",
            profile.to_str().unwrap(),
            "--agy-bin",
            binary.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doctor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["installation_ready"], true);
    assert_eq!(doctor["monitoring_prerequisites_ready"], false);
    assert_eq!(
        doctor["effective_assets"],
        root.join(".gemini/verij-monitoring-agy").to_str().unwrap()
    );
    let mut changed = status;
    changed["unrelated_added"] = json!(true);
    std::fs::write(&status_file, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(cli(&root, &["agent", "uninstall", "agy"]).status.success());
    let restored: Value = serde_json::from_slice(&std::fs::read(&status_file).unwrap()).unwrap();
    assert_eq!(restored["statusLine"], original["statusLine"]);
    assert_eq!(restored["unrelated_added"], true);
    let restored_hooks: Value =
        serde_json::from_slice(&std::fs::read(hooks_file).unwrap()).unwrap();
    assert_eq!(restored_hooks, json!({"user-hooks":{"Stop":[]}}));
    // Copied Magy commands still execute a neutral predecessor after uninstall.
    let copied: Value = serde_json::from_slice(
        &std::fs::read(profile.join("antigravity-cli/settings.json")).unwrap(),
    )
    .unwrap();
    let output = Command::new("/bin/sh")
        .args(["-c", copied["statusLine"]["command"].as_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"original");
    assert!(cli(&root, &args).status.success());
}

#[test]
fn edited_status_command_is_not_recursively_wrapped_or_deleted() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("agy");
    std::fs::write(&binary, "#!/bin/sh\nprintf '1.3.3\\n'\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "agent",
        "setup",
        "agy",
        "--agy-bin",
        binary.to_str().unwrap(),
    ];
    assert!(cli(root.path(), &args).status.success());
    let path = root.path().join(".gemini/antigravity-cli/settings.json");
    let mut status: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    status["statusLine"]["command"] = json!("printf user-edit");
    std::fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
    assert!(!cli(root.path(), &args).status.success());
    assert!(cli(root.path(), &["agent", "uninstall", "agy"])
        .status
        .success());
    let after: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(after["statusLine"]["command"], "printf user-edit");
}
