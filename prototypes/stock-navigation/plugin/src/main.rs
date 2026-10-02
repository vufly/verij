//! Verij-owned stock public-API probe. No Zellij transport extension.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::*;

const ROOT: &str = "/host/stock-state";
const CONTROL: &str = "verij_stock_navigation";
const BIND: &str = "verij_stock_bind";

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

fn write(name: &str, value: Value) {
    let target = format!("{ROOT}/{name}.json");
    let temporary = format!("{target}.tmp");
    if fs::write(&temporary, value.to_string()).is_ok() {
        let _ = fs::rename(temporary, target);
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Request {
    request_id: String,
    server_pid: u32,
    plugin_id: u32,
    client_id: u16,
    epoch: String,
    sequence: u64,
    operation: String,
    terminal_id: u32,
    target_session: Option<String>,
}

struct Pending {
    request: Request,
    polls: u8,
}

#[derive(Default)]
struct Probe {
    ids: Option<PluginIds>,
    epoch: String,
    ready: bool,
    sequence: u64,
    sample: u64,
    pending: Option<Pending>,
    input: String,
    session_name: Option<String>,
}

register_plugin!(Probe);

impl Probe {
    fn session(&self) -> Option<String> {
        self.session_name
            .clone()
            .or_else(|| {
                std::env::var("ZELLIJ_SESSION_NAME")
                    .ok()
                    .filter(|name| !name.is_empty())
            })
            .or_else(|| {
                get_session_list()
                    .ok()?
                    .live_sessions
                    .into_iter()
                    .find(|session| session.is_current_session)
                    .map(|session| session.name)
            })
    }

    fn observation(&mut self) -> Value {
        self.sample += 1;
        let ids = self.ids.as_ref().unwrap();
        let mut value = json!({"server_pid": ids.zellij_pid, "plugin_id": ids.plugin_id,
            "client_id": ids.client_id, "epoch": self.epoch, "sequence": self.sequence,
            "sample": self.sample, "time_ns": now(), "ready": self.ready});
        if self.ready {
            value["session"] = json!(self.session());
            match get_focused_pane_info() {
                Ok((tab, pane)) => {
                    value["focused_tab_index"] = json!(tab);
                    value["focused_pane"] = json!(pane.to_string());
                }
                Err(error) => value["focus_error"] = json!(error),
            }
        }
        value
    }

    fn publish(&mut self) {
        let value = self.observation();
        let ids = self.ids.as_ref().unwrap();
        write(
            &format!(
                "snapshot-{}-{}-{}-{}",
                ids.zellij_pid, ids.plugin_id, ids.client_id, self.epoch
            ),
            value,
        );
    }

    fn result(&mut self, request: Request, status: &str) {
        let observation = self.observation();
        write(
            &format!("result-{}", request.request_id),
            json!({"request": request,
            "status": status, "observation": observation,
            "whole_host_verified": false, "acknowledge_done": false,
            "guarantee": "stock plugin observation; no upstream execution barrier"}),
        );
    }

    fn target_exists(&self, pane: u32) -> bool {
        get_pane_info(PaneId::Terminal(pane))
            .map(|info| info.id == pane && !info.is_plugin)
            .unwrap_or(false)
    }

    fn request(&mut self, request: Request) {
        if !token(&request.request_id) {
            return;
        }
        let ids = self.ids.as_ref().unwrap();
        if ids.zellij_pid != request.server_pid
            || ids.plugin_id != request.plugin_id
            || ids.client_id != request.client_id
        {
            return; // CLI pipe is broadcast; only the exact plugin/client accepts it.
        }
        if self.epoch != request.epoch {
            self.result(request, "stale_plugin_epoch");
            return;
        }
        if !self.ready || !matches!(request.operation.as_str(), "focus" | "query" | "switch") {
            self.result(request, "unavailable_or_invalid");
            return;
        }
        if request.operation == "query" {
            if request.sequence != 0 {
                self.result(request, "invalid_sequence");
            } else {
                self.result(request, "observed");
            }
            return;
        }
        if request.sequence == 0 || request.sequence <= self.sequence {
            self.result(request, "superseded");
            return;
        }
        self.sequence = request.sequence;
        if let Some(previous) = self.pending.take() {
            self.result(previous.request, "superseded");
        }
        if request.operation == "switch" {
            let target = match &request.target_session {
                Some(name) if !name.is_empty() && name.len() <= 128 => name.clone(),
                _ => {
                    self.result(request, "invalid_target_session");
                    return;
                }
            };
            self.result(request.clone(), "switch_dispatched_unverified");
            switch_session_with_focus(&target, None, Some((request.terminal_id, false)));
            return;
        }
        if !self.target_exists(request.terminal_id) {
            self.result(request, "pane_missing");
            return;
        }
        focus_terminal_pane(request.terminal_id, false, false);
        self.pending = Some(Pending { request, polls: 0 });
    }
}

impl ZellijPlugin for Probe {
    fn load(&mut self, _: BTreeMap<String, String>) {
        self.ids = Some(get_plugin_ids());
        self.epoch = now(); // Verij-owned plugin-load epoch, not a Zellij connection nonce.
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::Timer,
            EventType::Key,
            EventType::SessionUpdate,
        ]);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
        ]);
        self.publish();
        set_timeout(0.2);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => self.ready = true,
            Event::SessionUpdate(sessions, _) => {
                if let Some(current) = sessions
                    .into_iter()
                    .find(|session| session.is_current_session)
                {
                    self.session_name = Some(current.name);
                }
            }
            Event::Key(key) => {
                let focused = self
                    .ids
                    .as_ref()
                    .map(|ids| {
                        get_focused_pane_info()
                            .ok()
                            .map(|(_, pane)| pane == PaneId::Plugin(ids.plugin_id))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if self.ready && focused {
                    match key.bare_key {
                        BareKey::Char(character)
                            if character.is_ascii_alphanumeric() || character == '-' =>
                        {
                            if self.input.len() < 96 {
                                self.input.push(character);
                            }
                        }
                        BareKey::Enter if token(&self.input) => {
                            let nonce = self.input.clone();
                            self.input.clear();
                            let observation = self.observation();
                            write(
                                &format!("binding-{nonce}"),
                                json!({"host_nonce": nonce,
                                "source": "focused_plugin_keyboard", "observation": observation, "time_ns": now()}),
                            );
                            hide_self(); // Leave the stock dialog through native restoration, not an assumed pane 0.
                        }
                        BareKey::Esc => self.input.clear(),
                        _ => {}
                    }
                }
            }
            Event::Timer(_) => {
                self.publish();
                if let Some(mut pending) = self.pending.take() {
                    let focused = get_focused_pane_info()
                        .ok()
                        .map(|(_, pane)| pane == PaneId::Terminal(pending.request.terminal_id))
                        .unwrap_or(false);
                    if focused {
                        self.result(pending.request, "inner_focus_observed");
                    } else if pending.polls >= 20 {
                        self.result(pending.request, "focus_unverified_timeout");
                    } else {
                        pending.polls += 1;
                        self.pending = Some(pending);
                    }
                }
                set_timeout(0.2);
            }
            _ => {}
        }
        false
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name == BIND {
            // Old keybind/CLI bind payloads cannot register a host. Registration
            // requires actual Event::Key input while this plugin is focused.
        } else if message.name == CONTROL {
            if let Some(payload) = &message.payload {
                if let Ok(request) = serde_json::from_str(payload) {
                    self.request(request);
                }
            }
        }
        if matches!(message.name.as_str(), BIND | CONTROL) {
            if let PipeSource::Cli(id) = message.source {
                unblock_cli_pipe_input(&id);
            }
        }
        false
    }

    fn render(&mut self, _: usize, _: usize) {
        println!(
            "STOCK VERIJ BINDING PROBE / client {}",
            self.ids.as_ref().map(|ids| ids.client_id).unwrap_or(0)
        );
        println!("Disposable registration dialog. Host controller types its nonce here.");
        println!("No production agent status or Done acknowledgement.");
    }
}
