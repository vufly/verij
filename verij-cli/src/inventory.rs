//! Native hydration validates OS incarnation; WASM exports only public locators.
use std::path::Path;
use verij_types::identity::SessionInstanceId;
use verij_types::SessionSnapshot;

pub fn hydrate(snapshot: &mut SessionSnapshot) {
    let Some(inventory) = snapshot.inventory.as_mut() else {
        return;
    };
    // Never trust process birth/instance claims supplied by a serialized file.
    inventory.session_instance_id = None;
    inventory.server_process = None;
    for pane in &mut inventory.panes {
        pane.pane_process = None;
    }
    if snapshot.needs_resurrection || inventory.schema_version != 1 {
        return;
    }
    let Ok(server) = crate::process::identity(inventory.producer.server_pid) else {
        return;
    };
    // A stale snapshot must not acquire the identity of a new process that
    // happens to reuse its exporter PID. Birth time is a rejection check only.
    if crate::process::started_at_ms(&server).map_or(true, |birth| inventory.exported_at_ms < birth)
    {
        return;
    }
    inventory.session_instance_id = Some(SessionInstanceId::from_process(&server));
    inventory.server_process = Some(server);
    for pane in &mut inventory.panes {
        if !pane.exited {
            pane.pane_process = pane
                .pane_pid
                .and_then(|pid| crate::process::identity(pid).ok());
        }
    }
}

/// Reads a bounded slice of session snapshot state files, advancing a persistent directory cursor.
/// Validates process birth and schema version, counting all inspected entries against `slice_limit`.
pub fn read_states_slice(
    directory: &Path,
    cursor: &mut Option<std::fs::ReadDir>,
    slice_limit: usize,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Vec<SessionSnapshot> {
    let mut snapshots = Vec::new();
    if cursor.is_none() {
        *cursor = std::fs::read_dir(directory).ok();
    }
    let Some(read_dir) = cursor.as_mut() else {
        return snapshots;
    };

    let mut inspected = 0;
    while inspected < slice_limit {
        if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
            break;
        }

        let entry = match read_dir.next() {
            Some(Ok(e)) => {
                inspected += 1;
                e
            }
            Some(Err(_)) => {
                inspected += 1;
                continue;
            }
            None => {
                *cursor = None;
                break;
            }
        };

        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }

        let Ok(bytes) = crate::agent_store::read_bounded_bytes(&path, 4 * 1024 * 1024) else {
            continue;
        };

        let Ok(mut snapshot) = serde_json::from_slice::<SessionSnapshot>(&bytes) else {
            continue;
        };
        hydrate(&mut snapshot);
        snapshots.push(snapshot);
    }
    snapshots
}

/// Does not launch session inventory subprocesses. Used by registration/store
/// reads and the development inventory command, independently of TUI refresh.
pub fn read_states(directory: &Path) -> Vec<SessionSnapshot> {
    let mut snapshots: Vec<SessionSnapshot> = Vec::new();
    let mut cursor = None;
    loop {
        let chunk = read_states_slice(directory, &mut cursor, 64, None);
        for snapshot in chunk {
            if let Some(previous) = snapshots
                .iter_mut()
                .find(|existing| existing.name == snapshot.name)
            {
                let age = |s: &SessionSnapshot| {
                    s.inventory.as_ref().map(|i| i.exported_at_ms).unwrap_or(0)
                };
                if age(&snapshot) >= age(previous) {
                    *previous = snapshot;
                }
            } else {
                snapshots.push(snapshot);
            }
        }
        if cursor.is_none() {
            break;
        }
    }
    snapshots.sort_by(|a, b| a.name.cmp(&b.name));
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(exported_at_ms: u64) -> SessionSnapshot {
        serde_json::from_value(json!({"name":"renamable","is_current":true,"tabs":[],"inventory": {
            "schema_version":1,"producer":{"server_pid":std::process::id(),"plugin_id":1,"client_id":1,"epoch":"probe"},
            "revision":1,"exported_at_ms":exported_at_ms,"session_instance_id":"forged-bare-pid",
            "server_process":null,"panes":[],"tabs":[],"capabilities":[]
        }})).unwrap()
    }

    #[test]
    fn old_export_cannot_acquire_new_pid_incarnation_and_rename_does_not_change_identity() {
        let mut old = snapshot(0);
        hydrate(&mut old);
        assert!(old.inventory.unwrap().session_instance_id.is_none());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let mut live = snapshot(now);
        hydrate(&mut live);
        let identity = live
            .inventory
            .as_ref()
            .unwrap()
            .session_instance_id
            .clone()
            .unwrap();
        assert_ne!(identity.0, "forged-bare-pid");
        live.name = "renamed".into();
        hydrate(&mut live);
        assert_eq!(live.inventory.unwrap().session_instance_id, Some(identity));
    }

    #[test]
    fn dead_server_and_unknown_wire_version_never_get_verified_identity() {
        let mut dead = snapshot(u64::MAX);
        dead.inventory.as_mut().unwrap().producer.server_pid = u32::MAX;
        hydrate(&mut dead);
        assert!(dead.inventory.unwrap().server_process.is_none());
        let mut unsupported = snapshot(u64::MAX);
        unsupported.inventory.as_mut().unwrap().schema_version = 2;
        hydrate(&mut unsupported);
        assert!(unsupported.inventory.unwrap().session_instance_id.is_none());
    }

    #[test]
    fn test_read_states_slice_with_cursor_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Write 3 session snapshots and 1 non-json file
        for i in 1..=3 {
            let snap = serde_json::json!({
                "name": format!("session-{}", i),
                "is_current": false,
                "tabs": [],
                "inventory": {
                    "schema_version": 1,
                    "producer": {"server_pid": std::process::id(), "plugin_id": 1, "client_id": 1, "epoch": "probe"},
                    "revision": 1,
                    "exported_at_ms": 1000,
                    "session_instance_id": null,
                    "server_process": null,
                    "panes": [],
                    "tabs": [],
                    "capabilities": []
                }
            });
            std::fs::write(
                root.join(format!("session-{}.json", i)),
                serde_json::to_vec(&snap).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(root.join("unrelated.txt"), b"ignore me").unwrap();

        let mut cursor = None;
        // Slice limit 2
        let chunk1 = read_states_slice(root, &mut cursor, 2, None);
        assert!(
            cursor.is_some(),
            "Cursor must remain active after partial slice"
        );

        let chunk2 = read_states_slice(root, &mut cursor, 2, None);
        let mut total = chunk1;
        total.extend(chunk2);

        // May need one more call to reach EOF if there were 4 files total (3 json + 1 txt)
        if cursor.is_some() {
            let chunk3 = read_states_slice(root, &mut cursor, 2, None);
            total.extend(chunk3);
            assert!(cursor.is_none(), "Cursor must be None at EOF");
        }

        assert_eq!(total.len(), 3);
        let names: Vec<String> = total.into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"session-1".to_string()));
        assert!(names.contains(&"session-2".to_string()));
        assert!(names.contains(&"session-3".to_string()));
    }
}
