use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn opencode_setup_and_doctor_accept_minor_and_patch_upgrades() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("opencode");
    let config = root.path().join("config");
    let cli = |action: &str| {
        Command::new(env!("CARGO_BIN_EXE_verij"))
            .args(["agent", action, "opencode", "--config-dir"])
            .arg(&config)
            .arg("--opencode-bin")
            .arg(&binary)
            .env("HOME", root.path())
            .env("VERIJ_AGENT_STATE_DIR", root.path().join("runtime"))
            .env("VERIJ_STATES_DIR", root.path().join("states"))
            .output()
            .unwrap()
    };
    for version in ["1.18.35", "1.19.0", "1.19.0-beta.1", "2.0.0"] {
        std::fs::write(&binary, format!("#!/bin/sh\nprintf '{version}\\n'\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let setup = cli("setup");
        if version == "2.0.0" {
            assert!(!setup.status.success());
        } else {
            assert!(
                setup.status.success(),
                "{version}: {}",
                String::from_utf8_lossy(&setup.stderr)
            );
            let installed: Value = serde_json::from_slice(&setup.stdout).unwrap();
            assert_eq!(installed["installed"], true);
            assert_eq!(installed["opencode_version"], version);
        }
        let doctor = cli("doctor");
        assert!(
            doctor.status.success(),
            "{}",
            String::from_utf8_lossy(&doctor.stderr)
        );
        let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
        assert_eq!(report["supported_versions"], json!(["1.x.x"]));
        assert_eq!(report["version_supported"], version != "2.0.0");
        assert_eq!(report["installation_ready"], version != "2.0.0");
    }
}
