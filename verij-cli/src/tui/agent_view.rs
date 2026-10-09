//! Pane-level presentation consumes normalized H1 execution facts only.
use crate::agent_watcher::AgentRecord;
use crate::config::AgentsConfig;
use std::collections::BTreeMap;
use verij_types::agent::{
    compute_display_status, sanitize_string, AgentKind, AgentStatus, InstanceAck,
};
use verij_types::identity::{AgentInstanceId, PaneKey};
use verij_types::SessionSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentView {
    pub instance: AgentInstanceId,
    pub pane: PaneKey,
    pub kind: AgentKind,
    pub title: String,
    pub pane_title: String,
    pub conversation_title: String,
    pub status: AgentStatus,
    pub detail: String,
    pub is_floating: bool,
    pub stacked: Option<bool>,
    pub completion_revision: u64,
    pub is_active: bool,
    pub is_synthetic: bool,
    pub navigable: bool,
    /// Last observed tab membership, used only for bounded Unknown grace.
    pub location: Option<(usize, Option<usize>)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSummary {
    pub total: usize,
    pub working: usize,
    pub needs_input: usize,
    pub done: usize,
    pub error: usize,
    pub unknown: usize,
    pub idle: usize,
}

impl AgentSummary {
    pub fn add(&mut self, status: AgentStatus) {
        self.total += 1;
        match status {
            AgentStatus::Working => self.working += 1,
            AgentStatus::NeedsInput => self.needs_input += 1,
            AgentStatus::Done => self.done += 1,
            AgentStatus::Error => self.error += 1,
            AgentStatus::Unknown => self.unknown += 1,
            AgentStatus::Idle => self.idle += 1,
        }
    }
    pub fn merge(&mut self, other: &Self) {
        self.total += other.total;
        self.working += other.working;
        self.needs_input += other.needs_input;
        self.done += other.done;
        self.error += other.error;
        self.unknown += other.unknown;
        self.idle += other.idle;
    }
}

/// Join known registered processes to exact current terminal topology. This is
/// presentation over H1 facts; it does not parse upstream events or invent Done.
pub fn build_views(
    sessions: &[SessionSnapshot],
    records: &[AgentRecord],
    acks: &BTreeMap<String, InstanceAck>,
    policy: &AgentsConfig,
    focus: Option<&PaneKey>,
) -> Vec<AgentView> {
    let mut views = Vec::new();
    for session in sessions {
        if session.needs_resurrection {
            continue;
        }
        let Some(inventory) = session.inventory.as_ref() else {
            continue;
        };
        let Some(instance) = inventory.session_instance_id.as_ref() else {
            continue;
        };
        for pane in inventory.panes.iter().filter(|pane| !pane.exited) {
            let key = PaneKey {
                session: instance.clone(),
                terminal: pane.terminal_id,
            };
            let mut candidates: Vec<_> = records
                .iter()
                .filter(|record| {
                    record.identity.pane_key == key
                        && crate::process::is_alive(&record.identity.process)
                })
                .collect();
            candidates.sort_by(|a, b| {
                a.identity
                    .agent_instance_id
                    .cmp(&b.identity.agent_instance_id)
            });
            candidates
                .dedup_by(|a, b| a.identity.agent_instance_id == b.identity.agent_instance_id);
            if candidates.is_empty() {
                continue;
            }
            let verified: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|record| {
                    match (
                        inventory.server_process.as_ref(),
                        pane.pane_process.as_ref(),
                    ) {
                        (Some(server), Some(leader)) => crate::process::verify_foreground(
                            server,
                            leader,
                            &record.identity.process,
                        )
                        .is_ok(),
                        _ => false,
                    }
                })
                .collect();
            let owner = if verified.len() == 1 {
                verified[0]
            } else {
                candidates[0]
            };
            let navigable = verified.len() == 1;
            let status = if navigable {
                compute_display_status(
                    &owner.state.reduced,
                    acks.get(&owner.identity.agent_instance_id.0),
                )
            } else {
                AgentStatus::Unknown
            };
            let conversation = sanitize_string(
                owner
                    .state
                    .reduced
                    .conversation_title
                    .as_deref()
                    .unwrap_or(""),
                256,
            );
            let pane_title = sanitize_string(&pane.title, 256);
            let mut title = policy.title_for(owner.identity.kind, &pane_title, &conversation);
            if owner.identity.is_synthetic {
                title = format!("[fixture] {title}");
            }
            let title = sanitize_string(&title, 256);
            views.push(AgentView {
                instance: owner.identity.agent_instance_id.clone(),
                pane: key.clone(),
                kind: owner.identity.kind,
                title,
                pane_title,
                conversation_title: conversation,
                status,
                detail: if navigable {
                    sanitize_string(owner.state.reduced.detail.as_deref().unwrap_or(""), 128)
                } else if verified.len() > 1 {
                    "ownership conflict".into()
                } else {
                    "foreground ownership unavailable".into()
                },
                is_floating: pane.is_floating,
                stacked: pane.stack_id.as_ref().map(|_| true),
                completion_revision: owner.state.reduced.completion_revision,
                is_active: navigable && focus == Some(&key),
                is_synthetic: owner.identity.is_synthetic,
                navigable,
                location: Some((pane.tab_position, pane.tab_id)),
            });
        }
    }
    views
}

#[derive(Default)]
pub struct PresentationCache {
    entries: BTreeMap<AgentInstanceId, (AgentView, std::time::Instant)>,
}

impl PresentationCache {
    pub fn refresh(
        &mut self,
        sessions: &[SessionSnapshot],
        records: &[AgentRecord],
        acks: &BTreeMap<String, InstanceAck>,
        policy: &AgentsConfig,
        focus: Option<&PaneKey>,
    ) -> Vec<AgentView> {
        self.refresh_at(
            sessions,
            records,
            acks,
            policy,
            focus,
            std::time::Instant::now(),
        )
    }

    pub fn refresh_at(
        &mut self,
        sessions: &[SessionSnapshot],
        records: &[AgentRecord],
        acks: &BTreeMap<String, InstanceAck>,
        policy: &AgentsConfig,
        focus: Option<&PaneKey>,
        now: std::time::Instant,
    ) -> Vec<AgentView> {
        let mut views = build_views(sessions, records, acks, policy, focus);
        for view in &views {
            self.entries
                .insert(view.instance.clone(), (view.clone(), now));
        }
        let missing: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, _)| !views.iter().any(|view| &view.instance == *id))
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect();
        for (id, (previous, seen)) in missing {
            let session_opt = sessions.iter().find(|session| {
                session
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.session_instance_id.as_ref())
                    == Some(&previous.pane.session)
            });
            let explicit_close = session_opt.is_some_and(|session| {
                session.needs_resurrection
                    || session
                        .inventory
                        .as_ref()
                        .and_then(|inventory| {
                            inventory
                                .panes
                                .iter()
                                .find(|pane| pane.terminal_id == previous.pane.terminal)
                        })
                        .is_some_and(|pane| pane.exited)
            });
            if explicit_close || views.iter().any(|view| view.pane == previous.pane) {
                self.entries.remove(&id);
                continue;
            }
            let live = records.iter().any(|record| {
                record.identity.agent_instance_id == id
                    && record.identity.pane_key == previous.pane
                    && crate::process::is_alive(&record.identity.process)
            });
            if live && now.duration_since(seen) < std::time::Duration::from_secs(3) {
                let mut uncertain = previous;
                uncertain.status = AgentStatus::Unknown;
                uncertain.is_active = false;
                uncertain.navigable = false;
                uncertain.detail = "pane topology uncertain".into();
                views.push(uncertain);
            } else {
                self.entries.remove(&id);
            }
        }
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use verij_types::agent::{AgentIdentity, AgentState, ReducedExecutionState};
    use verij_types::identity::{ProcessIdentity, SessionInstanceId, TerminalPaneId};
    use verij_types::inventory::{PaneSnapshot, SessionInventory, TabMetadata};
    use verij_types::TabSnapshot;

    #[cfg(target_os = "linux")]
    struct TestPtyChild {
        child: std::process::Child,
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

    #[cfg(target_os = "linux")]
    fn spawn_pty_foreground_process() -> Result<TestPtyChild, Box<dyn std::error::Error>> {
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
        if unsafe { libc::ptsname_r(master, buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0
        {
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

    fn make_test_inventory(
        session_id: &str,
        term_id: u32,
        server_process: Option<ProcessIdentity>,
        pane_process: Option<ProcessIdentity>,
        pane_title: &str,
        pane_exited: bool,
    ) -> SessionSnapshot {
        SessionSnapshot {
            name: "test-session".into(),
            is_current: false,
            tabs: vec![TabSnapshot {
                name: "main-tab".into(),
                position: 0,
                is_active: true,
            }],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: verij_types::identity::PluginContext {
                    server_pid: 1000,
                    plugin_id: 1,
                    client_id: 1,
                    epoch: "epoch-1".into(),
                },
                revision: 1,
                exported_at_ms: 1000,
                session_instance_id: Some(SessionInstanceId(session_id.into())),
                server_process,
                capabilities: vec![],
                tabs: vec![TabMetadata {
                    tab_id: 10,
                    position: 0,
                    floating_visible: false,
                    fullscreen_active: false,
                }],
                panes: vec![PaneSnapshot {
                    terminal_id: TerminalPaneId(term_id),
                    tab_id: Some(10),
                    tab_position: 0,
                    title: pane_title.into(),
                    is_floating: false,
                    is_suppressed: false,
                    is_fullscreen: false,
                    layer_focused: false,
                    exited: pane_exited,
                    pane_pid: None,
                    pane_process,
                    stack_id: None,
                }],
            }),
        }
    }

    fn make_test_record(
        agent_id: &str,
        session_id: &str,
        term_id: u32,
        kind: AgentKind,
        proc: ProcessIdentity,
        status: AgentStatus,
        detail: &str,
        completion_rev: u64,
        is_synthetic: bool,
    ) -> AgentRecord {
        AgentRecord {
            identity: AgentIdentity {
                schema_version: 1,
                agent_instance_id: AgentInstanceId(agent_id.into()),
                pane_key: PaneKey {
                    session: SessionInstanceId(session_id.into()),
                    terminal: TerminalPaneId(term_id),
                },
                kind,
                process: proc,
                registered_at_ms: 1000,
                is_synthetic,
                runner_id: None,
            },
            state: AgentState {
                schema_version: 1,
                agent_instance_id: AgentInstanceId(agent_id.into()),
                record_revision: 1,
                updated_at_ms: 1000,
                sources: BTreeMap::new(),
                reduced: ReducedExecutionState {
                    status,
                    current_turn_id: None,
                    current_turn_epoch: 1,
                    current_turn_revision: 1,
                    conversation_id: None,
                    conversation_title: Some("my conversation".into()),
                    pending_requests: vec![],
                    background_task_count: 0,
                    latest_error: None,
                    latest_completion: None,
                    completion_revision: completion_rev,
                    detail: Some(detail.into()),
                },
                completion_revision: completion_rev,
                completed_turn: None,
                completed_turn_epoch: 1,
                completed_turn_revision: 1,
                opencode_completions: BTreeMap::new(),
            },
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_real_foreground_ownership_verified() {
        let pty_child = spawn_pty_foreground_process().expect("failed to spawn PTY child process");
        let server_proc =
            crate::process::identity(std::process::id()).expect("server proc identity");
        let child_proc =
            crate::process::identity(pty_child.child.id()).expect("child proc identity");

        // Verify underlying process verification passes
        assert!(crate::process::verify_foreground(&server_proc, &child_proc, &child_proc).is_ok());

        let session = make_test_inventory(
            "sid-real",
            42,
            Some(server_proc),
            Some(child_proc.clone()),
            "active-term",
            false,
        );
        let record = make_test_record(
            "agent-real-1",
            "sid-real",
            42,
            AgentKind::Opencode,
            child_proc,
            AgentStatus::Working,
            "running test command",
            1,
            false,
        );

        let acks = BTreeMap::new();
        let policy = AgentsConfig::default();
        let pane_key = PaneKey {
            session: SessionInstanceId("sid-real".into()),
            terminal: TerminalPaneId(42),
        };

        // Test with focus matching
        let views = build_views(&[session], &[record], &acks, &policy, Some(&pane_key));
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert_eq!(view.instance.0, "agent-real-1");
        assert!(view.navigable);
        assert_eq!(view.status, AgentStatus::Working);
        assert_eq!(view.detail, "running test command");
        assert!(view.is_active);
        assert!(!view.is_synthetic);
    }

    #[test]
    fn test_fake_identities_not_verified() {
        let fake_proc = ProcessIdentity {
            pid: 999_999,
            start_jiffies: 12345,
            boot_id: "fake-boot".into(),
            uid: 1000,
        };
        let session = make_test_inventory(
            "sid-fake",
            42,
            Some(fake_proc.clone()),
            Some(fake_proc.clone()),
            "pane-fake",
            false,
        );
        // Candidate with synthetic flag and fake process
        let record = make_test_record(
            "agent-fake-1",
            "sid-fake",
            42,
            AgentKind::Agy,
            crate::process::identity(std::process::id()).unwrap(),
            AgentStatus::Working,
            "fake detail",
            1,
            true,
        );

        let acks = BTreeMap::new();
        let policy = AgentsConfig::default();
        let views = build_views(&[session.clone()], &[record.clone()], &acks, &policy, None);
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert!(!view.navigable);
        assert_eq!(view.status, AgentStatus::Unknown);
        assert_eq!(view.detail, "foreground ownership unavailable");
        assert!(!view.is_active);
        assert!(view.is_synthetic);
        assert!(view.title.starts_with("[fixture]"));
        let mut dead_fixture = record;
        dead_fixture.identity.process = fake_proc;
        assert!(
            build_views(&[session], &[dead_fixture], &acks, &policy, None).is_empty(),
            "synthetic semantics cannot bypass native process liveness"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_ownership_conflict_unknown_nonnavigable_oneperpane() {
        let child = spawn_pty_foreground_process().unwrap();
        let server = crate::process::identity(std::process::id()).unwrap();
        let live_proc = crate::process::identity(child.child.id()).unwrap();
        let session = make_test_inventory(
            "sid-conflict",
            10,
            Some(server),
            Some(live_proc.clone()),
            "pane-conflict",
            false,
        );
        // Two records targeting the exact same pane key
        let r1 = make_test_record(
            "ag-a",
            "sid-conflict",
            10,
            AgentKind::Opencode,
            live_proc.clone(),
            AgentStatus::Working,
            "detail-a",
            1,
            true,
        );
        let r2 = make_test_record(
            "ag-b",
            "sid-conflict",
            10,
            AgentKind::Opencode,
            live_proc,
            AgentStatus::Working,
            "detail-b",
            1,
            true,
        );

        let acks = BTreeMap::new();
        let policy = AgentsConfig::default();
        let views = build_views(&[session], &[r1, r2], &acks, &policy, None);

        // Exactly one row per pane
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert!(!view.navigable);
        assert_eq!(view.status, AgentStatus::Unknown);
        assert_eq!(view.detail, "ownership conflict");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_status_done_hostlocal_ack_not_global_state() {
        let child = spawn_pty_foreground_process().unwrap();
        let server = crate::process::identity(std::process::id()).unwrap();
        let proc = crate::process::identity(child.child.id()).unwrap();
        let session = make_test_inventory(
            "sid-done",
            20,
            Some(server),
            Some(proc.clone()),
            "pane-done",
            false,
        );
        let record = make_test_record(
            "ag-done",
            "sid-done",
            20,
            AgentKind::Opencode,
            proc,
            AgentStatus::Done,
            "completed work",
            3,
            false,
        );

        let policy = AgentsConfig::default();

        // 1. Without ack -> Done
        let acks_empty = BTreeMap::new();
        let views = build_views(
            &[session.clone()],
            &[record.clone()],
            &acks_empty,
            &policy,
            None,
        );
        assert!(views[0].navigable);
        assert_eq!(views[0].status, AgentStatus::Done);

        // Directly test compute_display_status host-local ack semantics
        let reduced = &record.state.reduced;
        assert_eq!(compute_display_status(reduced, None), AgentStatus::Done);

        // Ack with outdated revision (< 3) -> Still Done
        let stale_ack = InstanceAck {
            acknowledged_revision: 2,
            acknowledged_at_ms: 1000,
            turn_id: None,
            request_id: "req-1".into(),
        };
        assert_eq!(
            compute_display_status(reduced, Some(&stale_ack)),
            AgentStatus::Done
        );

        // Ack matching revision 3 -> Becomes Idle
        let match_ack = InstanceAck {
            acknowledged_revision: 3,
            acknowledged_at_ms: 2000,
            turn_id: None,
            request_id: "req-2".into(),
        };
        assert_eq!(
            compute_display_status(reduced, Some(&match_ack)),
            AgentStatus::Idle
        );

        // Ack exceeding revision 4 -> Becomes Idle
        let future_ack = InstanceAck {
            acknowledged_revision: 4,
            acknowledged_at_ms: 2000,
            turn_id: None,
            request_id: "req-3".into(),
        };
        assert_eq!(
            compute_display_status(reduced, Some(&future_ack)),
            AgentStatus::Idle
        );
        let host_a = BTreeMap::from([("ag-done".into(), match_ack)]);
        let read = build_views(
            &[session.clone()],
            &[record.clone()],
            &host_a,
            &policy,
            None,
        );
        let unread = build_views(
            &[session],
            &[record.clone()],
            &BTreeMap::new(),
            &policy,
            None,
        );
        assert_eq!(read[0].status, AgentStatus::Idle);
        assert_eq!(unread[0].status, AgentStatus::Done);
        assert_eq!(record.state.reduced.status, AgentStatus::Done);
    }

    #[test]
    fn test_titlesafe_bounded_configurable() {
        let proc = crate::process::identity(std::process::id()).unwrap();
        let long_pane = "p".repeat(500);
        let session = make_test_inventory("sid-title", 30, None, None, &long_pane, false);

        let long_conv = "c".repeat(500);
        let mut record = make_test_record(
            "ag-title",
            "sid-title",
            30,
            AgentKind::Opencode,
            proc,
            AgentStatus::Idle,
            "detail",
            1,
            true,
        );
        record.state.reduced.conversation_title = Some(long_conv);

        let policy = AgentsConfig::default();
        let views = build_views(&[session], &[record], &BTreeMap::new(), &policy, None);
        assert_eq!(views.len(), 1);
        let view = &views[0];

        // Ensure title is bounded to <= 256 characters
        assert!(view.title.len() <= 256);
        assert!(view.pane_title.len() <= 256);
        assert!(view.conversation_title.len() <= 256);
        assert!(view.title.starts_with("[fixture]"));
    }

    #[test]
    fn test_presentation_cache_grace_retains_live_unknown_and_expires() {
        let proc = crate::process::identity(std::process::id()).unwrap();
        let session = make_test_inventory("sid-cache", 50, None, None, "pane-cache", false);
        let record = make_test_record(
            "ag-cache-1",
            "sid-cache",
            50,
            AgentKind::Agy,
            proc,
            AgentStatus::Idle,
            "idle detail",
            1,
            true,
        );

        let mut cache = PresentationCache::default();
        let policy = AgentsConfig::default();
        let acks = BTreeMap::new();
        let t0 = std::time::Instant::now();

        // T0: Initial view created
        let v0 = cache.refresh_at(
            &[session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0,
        );
        assert_eq!(v0.len(), 1);
        assert_eq!(v0[0].instance.0, "ag-cache-1");

        // T0 + 1s: Topology uncertainty (session inventory temporarily missing the pane)
        let empty_session = SessionSnapshot {
            name: "test-session".into(),
            is_current: false,
            tabs: vec![],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: verij_types::identity::PluginContext {
                    server_pid: 1000,
                    plugin_id: 1,
                    client_id: 1,
                    epoch: "epoch-1".into(),
                },
                revision: 2,
                exported_at_ms: 2000,
                session_instance_id: Some(SessionInstanceId("sid-cache".into())),
                server_process: None,
                capabilities: vec![],
                tabs: vec![],
                panes: vec![], // Pane temporarily gone
            }),
        };

        let t1 = t0 + Duration::from_secs(1);
        let v1 = cache.refresh_at(
            &[empty_session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t1,
        );
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0].status, AgentStatus::Unknown);
        assert_eq!(v1[0].detail, "pane topology uncertain");
        assert!(!v1[0].navigable);

        // T0 + 2.5s: Still within 3s grace
        let t2 = t0 + Duration::from_millis(2500);
        let v2 = cache.refresh_at(
            &[empty_session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t2,
        );
        assert_eq!(v2.len(), 1);
        assert_eq!(v2[0].status, AgentStatus::Unknown);

        // T0 + 3.1s: Past 3s grace -> dropped from views and cache
        let t3 = t0 + Duration::from_millis(3100);
        let v3 = cache.refresh_at(&[empty_session], &[record], &acks, &policy, None, t3);
        assert_eq!(v3.len(), 0);
    }

    #[test]
    fn test_presentation_cache_explicit_death_and_replacement_remove_immediately() {
        let proc = crate::process::identity(std::process::id()).unwrap();
        let session = make_test_inventory("sid-imm", 60, None, None, "pane-imm", false);
        let record = make_test_record(
            "ag-imm-1",
            "sid-imm",
            60,
            AgentKind::Agy,
            proc.clone(),
            AgentStatus::Idle,
            "detail",
            1,
            true,
        );

        let policy = AgentsConfig::default();
        let acks = BTreeMap::new();
        let t0 = std::time::Instant::now();

        // 1. Explicit pane close removes immediately
        let mut cache = PresentationCache::default();
        cache.refresh_at(
            &[session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0,
        );

        let mut exited_session = session.clone();
        exited_session.inventory.as_mut().unwrap().panes[0].exited = true;
        let v_exited = cache.refresh_at(
            &[exited_session],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0 + Duration::from_millis(500),
        );
        assert_eq!(v_exited.len(), 0);

        // 2. Explicit session death (needs_resurrection) removes immediately
        let mut cache = PresentationCache::default();
        cache.refresh_at(
            &[session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0,
        );

        let mut dead_session = session.clone();
        dead_session.needs_resurrection = true;
        let v_dead = cache.refresh_at(
            &[dead_session],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0 + Duration::from_millis(500),
        );
        assert_eq!(v_dead.len(), 0);

        // 3. Replacement on same pane removes old agent immediately
        let mut cache = PresentationCache::default();
        cache.refresh_at(
            &[session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0,
        );

        let replacement_record = make_test_record(
            "ag-imm-2",
            "sid-imm",
            60,
            AgentKind::Opencode,
            proc,
            AgentStatus::Working,
            "new agent",
            1,
            true,
        );
        let v_repl = cache.refresh_at(
            &[session],
            &[replacement_record],
            &acks,
            &policy,
            None,
            t0 + Duration::from_millis(500),
        );
        assert_eq!(v_repl.len(), 1);
        assert_eq!(v_repl[0].instance.0, "ag-imm-2");
    }

    #[test]
    fn test_stale_topology_grace_preserves_done_ack() {
        let proc = crate::process::identity(std::process::id()).unwrap();
        let session = make_test_inventory("sid-ack-grace", 70, None, None, "pane-ack", false);
        let record = make_test_record(
            "ag-ack-1",
            "sid-ack-grace",
            70,
            AgentKind::Opencode,
            proc,
            AgentStatus::Done,
            "done work",
            2,
            true,
        );

        let policy = AgentsConfig::default();
        let mut acks = BTreeMap::new();
        acks.insert(
            "ag-ack-1".into(),
            InstanceAck {
                acknowledged_revision: 2,
                acknowledged_at_ms: 500,
                turn_id: None,
                request_id: "req-1".into(),
            },
        );

        let mut cache = PresentationCache::default();
        let t0 = std::time::Instant::now();

        // T0: Initial view
        cache.refresh_at(
            &[session.clone()],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0,
        );

        // T0 + 1s: Pane temporarily missing
        let mut empty_session = session.clone();
        empty_session.inventory.as_mut().unwrap().panes.clear();
        let v1 = cache.refresh_at(
            &[empty_session],
            &[record.clone()],
            &acks,
            &policy,
            None,
            t0 + Duration::from_secs(1),
        );
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0].status, AgentStatus::Unknown);

        // T0 + 2s: Topology restored; host ack in `acks` remains intact and status restored
        let v2 = cache.refresh_at(
            &[session],
            &[record],
            &acks,
            &policy,
            None,
            t0 + Duration::from_secs(2),
        );
        assert_eq!(v2.len(), 1);
        assert_eq!(v2[0].instance.0, "ag-ack-1");
    }

    #[test]
    fn test_agent_summary_add_and_merge() {
        let mut s1 = AgentSummary::default();
        s1.add(AgentStatus::Working);
        s1.add(AgentStatus::NeedsInput);
        s1.add(AgentStatus::Done);
        assert_eq!(s1.total, 3);
        assert_eq!(s1.working, 1);
        assert_eq!(s1.needs_input, 1);
        assert_eq!(s1.done, 1);

        let mut s2 = AgentSummary::default();
        s2.add(AgentStatus::Error);
        s2.add(AgentStatus::Unknown);
        s2.add(AgentStatus::Idle);
        assert_eq!(s2.total, 3);
        assert_eq!(s2.error, 1);
        assert_eq!(s2.unknown, 1);
        assert_eq!(s2.idle, 1);

        s1.merge(&s2);
        assert_eq!(s1.total, 6);
        assert_eq!(s1.working, 1);
        assert_eq!(s1.needs_input, 1);
        assert_eq!(s1.done, 1);
        assert_eq!(s1.error, 1);
        assert_eq!(s1.unknown, 1);
        assert_eq!(s1.idle, 1);
    }
}
