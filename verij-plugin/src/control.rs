//! Focused-keyboard registration and exact-recipient public stock control.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use verij_types::control::*;
use verij_types::identity::{valid_record_key, PluginContext, TerminalPaneId};
use verij_types::VERIJ_CONTROL_PIPE;
use zellij_tile::prelude::*;

pub struct Control {
    context: PluginContext,
    ready: bool,
    root: PathBuf,
    sequence: u64,
    input: String,
    session: Option<String>,
    pending: Option<(ControlRequest, u8)>,
    observer: bool,
}

fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Control {
    pub fn context(&self) -> &PluginContext {
        &self.context
    }
    pub fn load(configuration: &BTreeMap<String, String>) -> Self {
        let ids = get_plugin_ids();
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        subscribe(&[EventType::Timer, EventType::Key]);
        set_timeout(0.2);
        Self {
            context: PluginContext {
                server_pid: ids.zellij_pid,
                plugin_id: ids.plugin_id,
                client_id: ids.client_id,
                epoch,
            },
            ready: false,
            root: configuration
                .get("control_dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp/verij/control")),
            sequence: 0,
            input: String::new(),
            session: None,
            pending: None,
            observer: configuration
                .get("observer")
                .is_some_and(|value| value == "true"),
        }
    }

    fn write<T: serde::Serialize>(&self, name: &str, value: &T) {
        if std::fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let target = self.root.join(format!("{name}.json"));
        let temporary = self.root.join(format!(
            "{name}.{}.{}.tmp",
            self.context.client_id, self.context.epoch
        ));
        if let Ok(data) = serde_json::to_vec(value) {
            if std::fs::write(&temporary, data).is_ok()
                && std::fs::rename(&temporary, &target).is_err()
            {
                let _ = std::fs::remove_file(temporary);
            }
        }
    }

    fn observation(&self) -> FocusObservation {
        let focus = if self.ready {
            get_focused_pane_info().ok().map(|(_, pane)| pane)
        } else {
            None
        };
        let terminal = match focus {
            Some(PaneId::Terminal(id)) => Some(TerminalPaneId(id)),
            _ => None,
        };
        let focused_plugin = match focus {
            Some(PaneId::Plugin(id)) => Some(id),
            _ => None,
        };
        FocusObservation {
            context: self.context.clone(),
            session_name: self
                .session
                .clone()
                .or_else(|| std::env::var("ZELLIJ_SESSION_NAME").ok()),
            terminal,
            focused_plugin,
            queried_terminal: None,
            pane_pid: None,
            observed_at_ms: millis(),
            application_sequence: self.sequence,
        }
    }

    fn result(&self, request: ControlRequest, status: ControlStatus) {
        let name = format!("result-{}", request.request_id);
        let mut observation = self.observation();
        if let ControlOperation::Locate { terminal } = &request.operation {
            observation.queried_terminal = Some(*terminal);
            observation.pane_pid = get_pane_pid(PaneId::Terminal(terminal.0))
                .ok()
                .and_then(|pid| u32::try_from(pid).ok());
        } else if let Some(terminal) = observation.terminal {
            observation.queried_terminal = Some(terminal);
            observation.pane_pid = get_pane_pid(PaneId::Terminal(terminal.0))
                .ok()
                .and_then(|pid| u32::try_from(pid).ok());
        }
        self.write(
            &name,
            &ControlResult {
                request,
                status,
                observation: Some(observation),
                whole_host_verified: false,
                acknowledge_done: false,
            },
        );
    }

    pub fn update(&mut self, event: &Event) {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.ready = true;
                if self.observer {
                    hide_self();
                }
            }
            Event::SessionUpdate(sessions, _) => {
                if let Some(current) = sessions.iter().find(|session| session.is_current_session) {
                    self.session = Some(current.name.clone());
                }
            }
            Event::Key(key) if self.ready => {
                let focused = get_focused_pane_info()
                    .ok()
                    .is_some_and(|(_, pane)| pane == PaneId::Plugin(self.context.plugin_id));
                if !focused {
                    return;
                }
                match key.bare_key {
                    BareKey::Char(character)
                        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') =>
                    {
                        if self.input.len() < 128 {
                            self.input.push(character);
                        }
                    }
                    BareKey::Enter if valid_record_key(&self.input) => {
                        let nonce = std::mem::take(&mut self.input);
                        self.write(
                            &format!("binding-{nonce}"),
                            &Registration {
                                nonce,
                                context: self.context.clone(),
                                observation: self.observation(),
                                source: "focused_plugin_keyboard".into(),
                            },
                        );
                        hide_self();
                    }
                    BareKey::Esc => self.input.clear(),
                    _ => {}
                }
            }
            Event::Timer(_) => {
                if self.ready {
                    self.write(
                        &format!(
                            "focus-{}-{}-{}-{}",
                            self.context.server_pid,
                            self.context.plugin_id,
                            self.context.client_id,
                            self.context.epoch
                        ),
                        &self.observation(),
                    );
                }
                if let Some((request, polls)) = self.pending.take() {
                    let target = match request.operation {
                        ControlOperation::Focus { terminal } => Some(terminal),
                        _ => None,
                    };
                    if self.observation().terminal == target {
                        self.result(request, ControlStatus::InnerFocusObserved);
                    } else if polls >= 25 {
                        self.result(request, ControlStatus::Unavailable);
                    } else {
                        self.pending = Some((request, polls + 1));
                    }
                }
                set_timeout(0.2);
            }
            _ => {}
        }
    }

    /// Return true only for the structured protocol; legacy switch strings stay
    /// with the existing listener. Pipe source is never host ownership evidence.
    pub fn pipe(&mut self, message: &PipeMessage) -> bool {
        if message.name != VERIJ_CONTROL_PIPE {
            return false;
        }
        let Some(payload) = message
            .payload
            .as_deref()
            .filter(|value| value.trim_start().starts_with('{'))
        else {
            return false;
        };
        if let Ok(request) = serde_json::from_str::<ControlRequest>(payload) {
            self.request(request);
        }
        if let PipeSource::Cli(id) = &message.source {
            unblock_cli_pipe_input(id);
        }
        true
    }

    fn request(&mut self, request: ControlRequest) {
        if request.schema_version != SCHEMA_VERSION || !valid_record_key(&request.request_id) {
            return;
        }
        if request.context.server_pid != self.context.server_pid
            || request.context.plugin_id != self.context.plugin_id
            || request.context.client_id != self.context.client_id
        {
            return;
        }
        if request.context.epoch != self.context.epoch {
            self.result(request, ControlStatus::StaleContext);
            return;
        }
        if !self.ready {
            self.result(request, ControlStatus::Unavailable);
            return;
        }
        if matches!(
            request.operation,
            ControlOperation::Query | ControlOperation::Locate { .. }
        ) {
            if request.sequence != 0 {
                self.result(request, ControlStatus::Unavailable);
                return;
            }
            self.result(request, ControlStatus::Observed);
            return;
        }
        if request.sequence == 0 || request.sequence <= self.sequence {
            self.result(request, ControlStatus::Superseded);
            return;
        }
        self.sequence = request.sequence;
        if let Some((old, _)) = self.pending.take() {
            self.result(old, ControlStatus::Superseded);
        }
        match &request.operation {
            ControlOperation::Focus { terminal } => {
                if !get_pane_info(PaneId::Terminal(terminal.0))
                    .map(|pane| !pane.is_plugin && !pane.exited)
                    .unwrap_or(false)
                {
                    self.result(request, ControlStatus::PaneMissing);
                    return;
                }
                focus_terminal_pane(terminal.0, false, false);
                self.pending = Some((request, 0));
            }
            ControlOperation::Switch {
                session_name,
                terminal,
            } => {
                if session_name.is_empty()
                    || session_name.len() > 256
                    || session_name.chars().any(char::is_control)
                {
                    self.result(request, ControlStatus::Unavailable);
                    return;
                }
                let session = session_name.clone();
                let id = terminal.0;
                self.result(request, ControlStatus::SwitchDispatchedUnverified);
                switch_session_with_focus(&session, None, Some((id, false)));
            }
            ControlOperation::Query | ControlOperation::Locate { .. } => {}
        }
    }

    pub fn render(&self) {
        println!(
            "Verij stock client registration / client {}",
            self.context.client_id
        );
        println!("Verij host controller enters its binding nonce here.");
        println!("Registration is not a Done acknowledgement.");
    }
}
