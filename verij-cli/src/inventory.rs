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

/// Does not launch session inventory subprocesses. Used by registration/store
/// reads and the development inventory command, independently of TUI refresh.
pub fn read_states(directory: &Path) -> Vec<SessionSnapshot> {
    let mut snapshots: Vec<SessionSnapshot> = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return snapshots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.len() > 4 * 1024 * 1024 {
            continue;
        }
        let Ok(mut snapshot) = serde_json::from_slice::<SessionSnapshot>(&bytes) else {
            continue;
        };
        hydrate(&mut snapshot);
        // Prefer the latest producer observation; revisions from different
        // plugin load epochs are incomparable.
        if let Some(previous) = snapshots
            .iter_mut()
            .find(|existing| existing.name == snapshot.name)
        {
            let age =
                |s: &SessionSnapshot| s.inventory.as_ref().map(|i| i.exported_at_ms).unwrap_or(0);
            if age(&snapshot) >= age(previous) {
                *previous = snapshot;
            }
        } else {
            snapshots.push(snapshot);
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
}
