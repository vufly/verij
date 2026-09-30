//! CLI behavior against isolated XDG state and a fake Zellij executable.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

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
    if [ "${VERIJ_TEST_LIST_HANG:-}" = "1" ]; then exec sleep 60; fi
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
    if [ "$4" = "list-panes" ] && [ "${VERIJ_TEST_PANES_HANG:-}" = "1" ]; then exec sleep 60; fi
    if [ "$3" = "action" ] && [ "$4" = "rename-session" ]; then
      if [ "${VERIJ_TEST_ACTION_NOISE:-}" = "1" ]; then
        echo 'action stdout diagnostic'
        echo 'action stderr diagnostic' >&2
      fi
      if [ "${VERIJ_TEST_ACTION_FAIL:-}" = "1" ]; then exit 1; fi
      echo "$5 [Created 1m ago]" > "$VERIJ_TEST_SESSIONS"
    elif [ "$3" = "action" ] && [ "$4" = "override-layout" ]; then
      if [ "${VERIJ_TEST_RECOVER_FAIL:-}" = "1" ]; then
        echo 'layout recovery failed' >&2
        exit 1
      fi
      printf '%s\n' "$2 $5" > "$VERIJ_TEST_ACTION"
    elif [ "$2" = "inner" ]; then
      echo '[{"is_plugin":false,"tab_position":0},{"is_plugin":true,"tab_position":0,"is_suppressed":false,"is_floating":false}]'
    elif [ "${VERIJ_TEST_UNRELATED:-}" = "1" ]; then
      echo '[{"pane_command":"bash"}]'
    else
      echo '[{"pane_command":"verij ui"}]'
    fi
    ;;
  -s)
    if [ "${VERIJ_TEST_DETACH_HANG:-}" = "1" ]; then exec sleep 60; fi
    exit 0
    ;;
  --config)
    if [ "$3" = "attach" ]; then printf '%s\n' "$@" > "$VERIJ_TEST_ACTION"; fi
    exit 0
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
    fn action(&self) -> PathBuf { self.root.join("action") }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_verij"));
        command.args(args)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("VERIJ_TEST_SESSIONS", self.root.join("sessions"))
            .env("VERIJ_TEST_ACTION", self.action())
            .env("VERIJ_TEST_CLIENT_PID", self.root.join("client.pid"))
            .env_remove("ZELLIJ_CONFIG_FILE")
            .env_remove("ZELLIJ_CONFIG_DIR")
            .env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap()));
        for (key, value) in env { command.env(key, value); }
        let mut child = command.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("CLI hung: {}", String::from_utf8_lossy(&output.stderr));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }

    fn exited_host(&self, inner: bool) {
        fs::write(self.registry(), "[hosts.work]\nmarker_key = 'stable'\n").unwrap();
        fs::write(self.root.join("sessions"), "work [Created 1m ago] (EXITED - attach to resurrect)\ninner [Created 1m ago] (EXITED - attach to resurrect)\n").unwrap();
        let layouts = self.root.join("cache/zellij/contract_version_1/session_info");
        fs::create_dir_all(layouts.join("work")).unwrap();
        fs::write(layouts.join("work/session-layout.kdl"), "layout {\n pane command=\"verij\" name=\"Verij\" {\n args \"ui\"\n start_suspended true\n }\n pane command=\"zellij\" {\n args \"attach\" \"old\"\n }\n}\n").unwrap();
        fs::create_dir_all(self.root.join("config/zellij")).unwrap();
        fs::write(self.root.join("config/zellij/config.kdl"), "").unwrap();
        if inner {
            fs::write(self.attachments(), "[hosts.work]\nlast_session = 'inner'\n").unwrap();
            fs::create_dir_all(layouts.join("inner")).unwrap();
            fs::write(layouts.join("inner/session-layout.kdl"), "layout { pane { plugin location=\"zjstatus\"; } }\n").unwrap();
        }
    }

    fn fake_resurrection_client(&self) {
        let script = self.root.join("bin/script");
        fs::write(&script, r#"#!/bin/sh
echo $$ > "$VERIJ_TEST_CLIENT_PID"
if [ "${VERIJ_TEST_CLIENT_FAIL:-}" = "1" ]; then exit 7; fi
printf '%s\n' 'work [Created 1m ago] (EXITED - attach to resurrect)' 'inner [Created 1m ago]' > "$VERIJ_TEST_SESSIONS"
exec sleep 60
"#).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn assert_client_stopped(&self) {
        let pid: i32 = fs::read_to_string(self.root.join("client.pid")).unwrap().trim().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "temporary client {pid} leaked");
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
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

#[test]
fn recover_replaces_registered_live_host_layout_and_clears_its_marker() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.work]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.attachments(), "[hosts.work]\nlast_session = 'inner'\n").unwrap();
    fs::write(f.marker("stable"), "inner").unwrap();
    fs::write(f.root.join("sessions"), "work [Created 1m ago]\n").unwrap();
    fs::create_dir_all(f.root.join("config/verij")).unwrap();
    fs::write(f.root.join("config/verij/config.toml"), "[workspace]\nsidebar_width = '32%'\n").unwrap();

    let recovered = f.run(&["recover", "work"], &[]);
    assert!(recovered.status.success(), "{}", String::from_utf8_lossy(&recovered.stderr));
    assert_eq!(stdout(&recovered), "Recovered host 'work' layout\n");
    assert!(!f.marker("stable").exists());
    assert!(fs::read_to_string(f.attachments()).unwrap().contains("last_session = 'inner'"));
    let action = fs::read_to_string(f.action()).unwrap();
    let layout = action.strip_prefix("work ").unwrap().trim();
    let layout = fs::read_to_string(layout).unwrap();
    assert!(layout.contains("pane size=\"32%\" name=\"Verij\""), "{layout}");
    assert!(layout.contains("pane name=\"Workspace\" borderless=true"));
}

#[test]
fn recover_uses_current_host_without_argument_and_rejects_invalid_targets() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.work]\nmarker_key = 'stable'\n[hosts.exited]\nmarker_key = 'saved'\n").unwrap();
    fs::write(f.root.join("sessions"), "work [Created 1m ago]\nexited [Created 2m ago] (EXITED - attach to resurrect)\nunregistered [Created 3m ago]\n").unwrap();

    assert!(f.run(&["recover"], &[("ZELLIJ_SESSION_NAME", "work")]).status.success());
    assert!(!f.run(&["recover"], &[]).status.success());
    assert!(!f.run(&["recover", "missing"], &[]).status.success());
    assert!(!f.run(&["recover", "unregistered"], &[]).status.success());
    assert!(!f.run(&["recover", "exited"], &[]).status.success());
}

#[test]
fn failed_recovery_preserves_workspace_marker() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.work]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.marker("stable"), "inner").unwrap();
    fs::write(f.root.join("sessions"), "work [Created 1m ago]\n").unwrap();

    assert!(!f.run(&["recover", "work"], &[("VERIJ_TEST_RECOVER_FAIL", "1")]).status.success());
    assert_eq!(fs::read_to_string(f.marker("stable")).unwrap(), "inner");
}

#[test]
fn exited_host_attaches_directly_with_saved_commands_enabled() {
    let f = Fixture::new();
    f.exited_host(false);
    f.fake_resurrection_client();
    let attached = f.run(&["attach", "-c", "work"], &[]);
    assert!(attached.status.success(), "{}", String::from_utf8_lossy(&attached.stderr));
    assert!(fs::read_to_string(f.action()).unwrap().ends_with("attach\n--force-run-commands\nwork\n"));
    assert!(!f.root.join("client.pid").exists(), "host must use caller's terminal, not a temporary client");
    let layout = fs::read_to_string(f.root.join("cache/zellij/contract_version_1/session_info/work/session-layout.kdl")).unwrap();
    assert!(layout.contains("start_suspended false"));
}

#[test]
fn lingering_inner_client_is_reaped_before_attaching_host() {
    let f = Fixture::new();
    f.exited_host(true);
    f.fake_resurrection_client();
    let attached = f.run(&["attach", "-c", "work"], &[]);
    assert!(attached.status.success(), "{}", String::from_utf8_lossy(&attached.stderr));
    f.assert_client_stopped();
    assert!(attached.stdout.is_empty(), "resurrection must not write into the sidebar terminal");
    assert!(attached.stderr.is_empty(), "resurrection must not write into the sidebar terminal");
    let layout = fs::read_to_string(f.root.join("cache/zellij/contract_version_1/session_info/work/session-layout.kdl")).unwrap();
    assert!(layout.contains("args \"attach\" \"--force-run-commands\" \"inner\""));
}

#[test]
fn blocked_resurrection_queries_fail_and_reap_the_client() {
    for variable in ["VERIJ_TEST_PANES_HANG", "VERIJ_TEST_DETACH_HANG"] {
        let f = Fixture::new();
        f.exited_host(true);
        f.fake_resurrection_client();
        let attached = f.run(&["attach", "-c", "work"], &[(variable, "1")]);
        assert!(!attached.status.success());
        assert!(String::from_utf8_lossy(&attached.stderr).contains("timed out"), "{}", String::from_utf8_lossy(&attached.stderr));
        f.assert_client_stopped();
    }
}

#[test]
fn failed_inner_client_reports_failure_without_waiting_for_readiness_timeout() {
    let f = Fixture::new();
    f.exited_host(true);
    f.fake_resurrection_client();
    let started = Instant::now();
    let attached = f.run(&["attach", "-c", "work"], &[("VERIJ_TEST_CLIENT_FAIL", "1")]);
    assert!(!attached.status.success());
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(String::from_utf8_lossy(&attached.stderr).contains("exited before its layout was ready"));
    f.assert_client_stopped();
}

#[test]
fn blocked_inventory_query_fails_instead_of_hanging_attach() {
    let f = Fixture::new();
    let attached = f.run(&["attach", "-c", "work"], &[("VERIJ_TEST_LIST_HANG", "1")]);
    assert!(!attached.status.success());
    assert!(String::from_utf8_lossy(&attached.stderr).contains("timed out"));
}

#[test]
fn sidebar_actions_capture_subprocess_output_and_report_failures() {
    let f = Fixture::new();
    fs::write(f.registry(), "[hosts.old]\nmarker_key = 'stable'\n").unwrap();
    fs::write(f.root.join("sessions"), "old [Created 1m ago]\n").unwrap();
    let renamed = f.run(&["rename", "old", "new"], &[("VERIJ_TEST_ACTION_NOISE", "1")]);
    assert!(renamed.status.success(), "{}", String::from_utf8_lossy(&renamed.stderr));
    assert_eq!(stdout(&renamed), "Renamed host 'old' to 'new'\n");
    assert!(renamed.stderr.is_empty());

    let failed = f.run(&["rename", "new", "old"], &[
        ("VERIJ_TEST_ACTION_NOISE", "1"), ("VERIJ_TEST_ACTION_FAIL", "1"),
    ]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(error.starts_with("Error: Zellij "), "{error}");
    assert!(error.contains("action stderr diagnostic"));
    assert!(fs::read_to_string(f.registry()).unwrap().contains("[hosts.new]"));
}
