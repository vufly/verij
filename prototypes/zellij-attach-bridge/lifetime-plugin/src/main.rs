//! Disposable observer: record each actual per-client plugin load without permissions.
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::*;

#[derive(Default)]
struct Probe {
    marker: String,
}

register_plugin!(Probe);

impl ZellijPlugin for Probe {
    fn load(&mut self, _: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let value = serde_json::json!({
            "plugin_id": ids.plugin_id, "client_id": ids.client_id,
            "server_pid": ids.zellij_pid, "load_nonce": nonce.to_string(),
        });
        self.marker = value.to_string();
        // The harness launches the plugin with its own scratch root as /host.
        // No command execution, permission grant, or shared user directory.
        let path = format!(
            "/host/plugin-loads/{}-{}-{nonce}.json",
            ids.plugin_id, ids.client_id
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", self.marker).unwrap();
    }

    fn render(&mut self, _: usize, _: usize) {
        println!("LIFETIME_PROBE {}", self.marker);
    }
}
