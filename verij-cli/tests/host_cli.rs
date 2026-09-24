//! CLI behavior against isolated XDG state and a fake Zellij executable.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("verij-host-cli-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("state/verij")).unwrap();
        fs::create_dir_all(root.join("runtime/verij")).unwrap();
        let zellij = root.join("bin/zellij");
        fs::write(&zellij, r#"#!/bin/sh
case "$1" in
  list-sessions)
    if [ "${VERIJ_TEST_LIST_FAIL:-}" = "1" ]; then
      echo 'inventory unavailable' >&2
      exit 1
    fi
    if [ "${VERIJ_TEST_NO_SESSIONS:-}" = "1" ]; then
      echo 'No active zellij sessions found.' >&2
      exit 1
    fi
    cat "$VERIJ_TEST_SESSIONS"
    ;;
  --session)
    if [ "$3" = "action" ] && [ "$4" = "rename-session" ]; then
      echo "$5 [Created 1m ago]" > "$VERIJ_TEST_SESSIONS"
    elif [ "${VERIJ_TEST_UNRELATED:-}" = "1" ]; then
      echo '[{"pane_command":"bash"}]'
    else
      echo '[{"pane_command":"verij ui"}]'
    fi
    ;;
  delete-session)
    if [ "${VERIJ_TEST_DELETE_FAIL:-}" = "1" ]; then
      echo 'deletion failed' >&2
      exit 1
    fi
    : > "$VERIJ_TEST_SESSIONS"
    ;;
  *) exit 2 ;;
esac
"#).unwrap();
        fs::set_permissions(&zellij, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("sessions"), "").unwrap();
        Self { root }
    }

    fn registry(&self) -> PathBuf { self.root.join("state/verij/host-registry.toml") }
    fn attachments(&self) -> PathBuf { self.root.join("state/verij/hosts.toml") }
    fn marker(&self, key: &str) -> PathBuf { self.root.join(format!("runtime/verij/workspace-{key}.session")) }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_verij"));
        command.args(args)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("VERIJ_TEST_SESSIONS", self.root.join("sessions"))
            .env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap()));
        for (key, value) in env { command.env(key, value); }
        command.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { fs::remove_dir_all(&self.root).unwrap(); }
}

fn stdout(output: &Output) -> String { String::from_utf8(output.stdout.clone()).unwrap() }

#[test]
fn list_and_prune_preserve_live_and_exited_hosts() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.live]\nmarker_key = 'one'\n[hosts.exited]\nmarker_key = 'two'\n[hosts.gone]\nmarker_key = 'three'\n").unwrap();
    fs::write(f.attachments(), "[hosts.gone]\nlast_session = 'inner'\n[hosts.orphan]\nlast_session = 'inner'\n").unwrap();
    fs::write(f.marker("three"), "inner").unwrap();
    fs::write(f.root.join("sessions"), "live [Created 1m ago]\nexited [Created 2m ago] (EXITED - attach to resurrect)\n").unwrap();

    let listed = f.run(&["list"], &[]);
    assert!(listed.status.success());
    assert_eq!(stdout(&listed), "exited\texited\ngone\tmissing\nlive\tlive\n");
    assert!(!f.run(&["list"], &[("VERIJ_TEST_LIST_FAIL", "1")]).status.success());
    assert_eq!(stdout(&f.run(&["list", "--live"], &[])), "live\tlive\n");
    assert_eq!(stdout(&f.run(&["list", "--exited", "--missing"], &[])), "exited\texited\ngone\tmissing\n");
    let shown = f.run(&["show", "gone", "--check"], &[]);
    assert!(shown.status.success());
    assert!(stdout(&shown).contains("Last session: inner\nZellij: missing"));

    let dry_run = f.run(&["prune", "--dry-run"], &[]);
    assert!(dry_run.status.success());
    assert!(stdout(&dry_run).contains("Would prune gone\nWould prune orphan\n"));
    assert!(fs::read_to_string(f.registry()).unwrap().contains("gone"));

    let failed = f.run(&["prune"], &[("VERIJ_TEST_LIST_FAIL", "1")]);
    assert!(!failed.status.success());
    assert!(fs::read_to_string(f.registry()).unwrap().contains("gone"));
    let pruned = f.run(&["prune"], &[]);
    assert!(pruned.status.success());
    assert!(stdout(&pruned).contains("2 host record(s) pruned"));
    assert!(!f.marker("three").exists());
    assert_eq!(stdout(&f.run(&["list"], &[])), "exited\texited\nlive\tlive\n");
    assert!(!fs::read_to_string(f.attachments()).unwrap().contains("orphan"));
}

#[test]
fn delete_protects_live_or_unrelated_sessions_and_preserves_state_on_failure() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.work]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.attachments(), "[hosts.work]\nlast_session = 'inner'\n").unwrap();
    fs::write(f.marker("stable"), "inner").unwrap();
    fs::write(f.root.join("sessions"), "work [Created 1m ago]\n").unwrap();

    assert!(!f.run(&["delete", "work"], &[]).status.success());
    assert!(!f.run(&["delete", "work", "--zellij"], &[]).status.success());
    assert!(!f.run(&["delete", "work", "--force"], &[]).status.success());
    assert!(!f.run(&["delete", "work", "--zellij", "--force"], &[("VERIJ_TEST_LIST_FAIL", "1")]).status.success());
    assert!(!f.run(&["delete", "work", "--zellij", "--force"], &[("VERIJ_TEST_UNRELATED", "1")]).status.success());
    assert!(!f.run(&["delete", "work", "--zellij", "--force"], &[("VERIJ_TEST_DELETE_FAIL", "1")]).status.success());
    assert!(fs::read_to_string(f.registry()).unwrap().contains("work"));
    assert!(fs::read_to_string(f.attachments()).unwrap().contains("work"));
    assert!(f.marker("stable").exists());

    let removed = f.run(&["delete", "work", "--zellij", "--force"], &[]);
    assert!(removed.status.success(), "{}", String::from_utf8_lossy(&removed.stderr));
    assert_eq!(stdout(&removed), "Deleted host 'work'\n");
    assert!(!fs::read_to_string(f.registry()).unwrap().contains("work"));
    assert!(!f.marker("stable").exists());
}

#[test]
fn exited_host_is_deleted_without_force_when_layout_verifies_identity() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.work]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.root.join("sessions"), "work [Created 1m ago] (EXITED - attach to resurrect)\n").unwrap();
    let layout = f.root.join("cache/zellij/contract_version_1/session_info/work/session-layout.kdl");
    fs::create_dir_all(layout.parent().unwrap()).unwrap();
    fs::write(layout, "layout { tab name=\"Verij Host\" { pane command=\"verij\" { args \"ui\" } } }").unwrap();

    assert!(f.run(&["delete", "work", "--zellij"], &[]).status.success());
    assert!(!fs::read_to_string(f.registry()).unwrap().contains("work"));
}

#[test]
fn empty_zellij_inventory_can_be_pruned() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.gone]\nmarker_key = 'stable'\n").unwrap();
    let pruned = f.run(&["prune"], &[("VERIJ_TEST_NO_SESSIONS", "1")]);
    assert!(pruned.status.success(), "{}", String::from_utf8_lossy(&pruned.stderr));
    assert_eq!(stdout(&pruned), "Pruned gone\n1 host record(s) pruned\n");
}

#[test]
fn rename_uses_live_host_path_and_preserves_marker_key_and_attachment() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.old]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.attachments(), "[hosts.old]\nlast_session = 'inner'\n").unwrap();
    fs::write(f.marker("stable"), "inner").unwrap();
    fs::write(f.root.join("sessions"), "old [Created 1m ago]\n").unwrap();

    let renamed = f.run(&["rename", "old", "new"], &[]);
    assert!(renamed.status.success(), "{}", String::from_utf8_lossy(&renamed.stderr));
    assert_eq!(stdout(&f.run(&["list", "--live"], &[])), "new\tlive\n");
    assert!(fs::read_to_string(f.registry()).unwrap().contains("marker_key = \"stable\""));
    assert!(fs::read_to_string(f.attachments()).unwrap().contains("[hosts.new]"));
    assert!(f.marker("stable").exists());
}
