use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    binary: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir(&config).unwrap();
        let binary = root.path().join("opencode");
        let f = Self {
            root,
            config,
            binary,
        };
        f.version("opencode v2.1.2");
        f
    }
    fn version(&self, version: &str) {
        std::fs::write(
            &self.binary,
            format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"),
        )
        .unwrap();
        std::fs::set_permissions(&self.binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn command(&self, action: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_verij"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.path())
            .env("VERIJ_AGENT_STATE_DIR", self.root.path().join("runtime"))
            .env("VERIJ_STATES_DIR", self.root.path().join("states"))
            .args(["agent", action, "opencode", "--config-dir"])
            .arg(&self.config)
            .arg("--opencode-bin")
            .arg(&self.binary);
        cmd
    }
    fn setup(&self) -> Output {
        self.command("setup")
            .arg("--experimental-live-status")
            .output()
            .unwrap()
    }
    fn assets(&self) -> PathBuf {
        self.config.join("verij-monitoring-v2-live")
    }
    fn doctor(&self) -> Value {
        parse(self.command("doctor").output().unwrap())
    }
}
fn parse(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn failure(output: Output, message: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn v2_requires_opt_in_and_reports_only_experimental_live_capabilities() {
    let f = Fixture::new();
    failure(
        f.command("setup").output().unwrap(),
        "--experimental-live-status",
    );
    assert!(!f.config.join("cli.json").exists());
    let setup = parse(f.setup());
    assert_eq!(setup["mode"], "live_status");
    assert_eq!(setup["bridge_version"], 2);
    assert_eq!(setup["opencode_version"], "2.1.2");
    assert_eq!(setup["experimental"], true);
    let doctor = f.doctor();
    assert_eq!(doctor["installation_ready"], true);
    assert_eq!(doctor["monitoring_prerequisites_ready"], false);
    assert_eq!(doctor["live_records"], 0);
    assert_eq!(doctor["mode"], "live_status");
    assert!(!doctor["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("aggregate_completion")));
    assert!(!doctor["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("terminal_failure")));
    assert!(!f.config.join("tui.json").exists());
    let cli: Value =
        serde_json::from_str(&std::fs::read_to_string(f.config.join("cli.json")).unwrap()).unwrap();
    assert_eq!(cli["plugins"][0]["package"], json!(f.assets()));
    assert_eq!(cli["plugins"][0]["options"]["experimentalLiveStatus"], true);
}

#[test]
fn cli_jsonc_comments_options_and_directives_survive_idempotent_setup_uninstall() {
    let f = Fixture::new();
    let original = "{\n// user comment\n\"theme\":\"mine\",\"plugins\":[/* other */{\"package\":\"other\",\"options\":{\"x\":1}},\"-unrelated\",],\"keybinds\":{\"leader\":\"ctrl+x\"},}";
    std::fs::write(f.config.join("cli.json"), original).unwrap();
    parse(f.setup());
    let first = std::fs::read_to_string(f.config.join("cli.json")).unwrap();
    assert!(first.contains("// user comment"));
    assert!(first.contains("/* other */{\"package\":\"other\",\"options\":{\"x\":1}}"));
    parse(f.setup());
    assert_eq!(
        std::fs::read_to_string(f.config.join("cli.json")).unwrap(),
        first
    );
    parse(f.command("uninstall").output().unwrap());
    let removed = std::fs::read_to_string(f.config.join("cli.json")).unwrap();
    assert!(removed.contains("// user comment"));
    assert!(removed.contains("\"-unrelated\""));
    assert!(!removed.contains("verij-monitoring-v2-live"));
    assert!(!f.assets().exists());
}

#[test]
fn setup_refuses_to_suppress_first_start_migration() {
    for name in ["tui.json", "tui.jsonc", "kv.json"] {
        let f = Fixture::new();
        std::fs::write(f.config.join(name), "{}").unwrap();
        failure(f.setup(), "migration/review");
        assert!(!f.config.join("cli.json").exists());
        assert!(!f.assets().exists());
        assert_eq!(f.doctor()["migration_pending"], true);
    }
}

#[test]
fn v1_coexists_and_only_verified_migrated_entry_is_replaced() {
    let f = Fixture::new();
    f.version("1.19.0");
    parse(f.command("setup").output().unwrap());
    let tui = std::fs::read(f.config.join("tui.json")).unwrap();
    let v1 = f.config.join("verij-monitoring/bridge.mjs");
    let content = std::fs::read(&v1).unwrap();
    std::fs::write(
        f.config.join("cli.json"),
        json!({"plugins":[{"package":v1,"options":{"reporter":"old"}}, "other"]}).to_string(),
    )
    .unwrap();
    f.version("opencode v2.1.2");
    parse(f.setup());
    assert_eq!(std::fs::read(f.config.join("tui.json")).unwrap(), tui);
    assert_eq!(std::fs::read(&v1).unwrap(), content);
    let cli: Value =
        serde_json::from_slice(&std::fs::read(f.config.join("cli.json")).unwrap()).unwrap();
    assert_eq!(cli["plugins"].as_array().unwrap().len(), 2);
    assert_eq!(cli["plugins"][0], "other");
    std::fs::remove_file(&f.binary).unwrap(); // Uninstall cannot depend on executable availability.
    parse(f.command("uninstall").output().unwrap());
    assert_eq!(std::fs::read(f.config.join("tui.json")).unwrap(), tui);
    assert!(v1.exists());
}

#[test]
fn modified_assets_are_neither_overwritten_nor_deleted() {
    let f = Fixture::new();
    parse(f.setup());
    let file = f.assets().join("core.mjs");
    std::fs::write(&file, "user work").unwrap();
    failure(f.setup(), "modified/unowned");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "user work");
    assert_eq!(f.doctor()["assets_match"], false);
    let removed = parse(f.command("uninstall").output().unwrap());
    assert_eq!(removed["preserved"], json!([file]));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "user work");
}

#[test]
fn disabled_choices_and_inline_overrides_are_not_overridden() {
    for directive in ["-verij.monitoring.v2.live", "-*", "-verij.*"] {
        let f = Fixture::new();
        let text = json!({"plugins":[directive]}).to_string();
        std::fs::write(f.config.join("cli.json"), &text).unwrap();
        failure(f.setup(), "disabled");
        assert_eq!(
            std::fs::read_to_string(f.config.join("cli.json")).unwrap(),
            text
        );
        assert_eq!(f.doctor()["disabled"], true);
        assert!(!f.assets().exists());
    }
    let f = Fixture::new();
    let output = f
        .command("setup")
        .arg("--experimental-live-status")
        .env("OPENCODE_CLI_CONFIG_CONTENT", "{\"plugins\":[]}")
        .output()
        .unwrap();
    failure(output, "OPENCODE_CLI_CONFIG_CONTENT");
    assert!(!f.assets().exists());
    parse(f.setup());
    let doctor = parse(
        f.command("doctor")
            .env("OPENCODE_CLI_CONFIG_CONTENT", "{\"plugins\":[]}")
            .output()
            .unwrap(),
    );
    assert_eq!(doctor["inline_plugins_override"], true);
    assert_eq!(doctor["installation_ready"], false);
}

#[test]
fn output_normalization_does_not_widen_version_or_schema_admission() {
    let f = Fixture::new();
    for version in ["2.0.31", "3.0.0", "1.19.0", "opencode v2.0.31"] {
        f.version(version);
        failure(f.setup(), "requires OpenCode 2.x");
    }
    for version in [
        "opencode v2.1.2\nbanner",
        "OpenCode 2.1.2",
        "v2.1.2",
        "2.1.2-",
    ] {
        f.version(version);
        failure(f.setup(), "invalid OpenCode version output");
    }
    for version in ["opencode v2.0.32", "2.1.2", "opencode v2.1.2+build.7"] {
        f.version(version);
        parse(f.setup());
    }
}

#[test]
fn ambiguous_legacy_references_and_invalid_config_are_preserved() {
    for text in ["{broken", "{\"plugins\":{}}"] {
        let f = Fixture::new();
        std::fs::write(f.config.join("cli.json"), text).unwrap();
        assert!(!f.setup().status.success());
        assert_eq!(
            std::fs::read_to_string(f.config.join("cli.json")).unwrap(),
            text
        );
        assert!(!f.assets().exists());
    }
    let f = Fixture::new();
    let text =
        json!({"plugins":[{"package":f.config.join("verij-monitoring/bridge.mjs")}]}).to_string();
    std::fs::write(f.config.join("cli.json"), &text).unwrap();
    failure(f.setup(), "no verifiable owned asset");
    assert_eq!(
        std::fs::read_to_string(f.config.join("cli.json")).unwrap(),
        text
    );
}

#[test]
fn v2_config_resolution_ignores_v1_tui_override() {
    let f = Fixture::new();
    let foreign = f.root.path().join("foreign.json");
    std::fs::write(&foreign, "{}").unwrap();
    let output = f
        .command("setup")
        .arg("--experimental-live-status")
        .env("OPENCODE_TUI_CONFIG", &foreign)
        .output()
        .unwrap();
    parse(output);
    assert_eq!(std::fs::read_to_string(foreign).unwrap(), "{}");
    assert!(f.config.join("cli.json").exists());
}
