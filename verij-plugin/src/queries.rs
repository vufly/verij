//! Fallible wrappers for stock SDK queries whose implementation unwraps replies.
//! A denied or missing reply must not trap WASM while plugin state is borrowed.
use zellij_tile::prelude::*;
use zellij_tile::shim::plugin_api::plugin_command::{
    get_focused_pane_info_response, ProtobufGetFocusedPaneInfoResponse, ProtobufGetPaneInfoResponse,
};
#[cfg(target_arch = "wasm32")]
use zellij_tile::shim::plugin_api::plugin_command::{
    get_session_list_response, ProtobufGetSessionListResponse, ProtobufGetTabInfoResponse,
    ProtobufPluginCommand,
};
use zellij_tile::shim::prost::Message;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "zellij")]
extern "C" {
    fn host_run_plugin_command();
}

fn query<R: Message + Default>(command: PluginCommand) -> Result<R, String> {
    query_with_empty(command, false)
}

fn query_with_empty<R: Message + Default>(
    command: PluginCommand,
    allow_empty: bool,
) -> Result<R, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let command: ProtobufPluginCommand = command
            .try_into()
            .map_err(|error| format!("Invalid SDK command: {error}"))?;
        zellij_tile::shim::object_to_stdout(&command.encode_to_vec());
        unsafe { host_run_plugin_command() };
        let bytes = zellij_tile::shim::bytes_from_stdin()
            .map_err(|error| format!("Missing SDK reply: {error}"))?;
        // Optional-object responses encode an absent object as zero bytes.
        // Reading stdin still must succeed; absence is not a transport failure.
        if allow_empty && bytes.is_empty() {
            Ok(R::default())
        } else {
            decode_reply(&bytes)
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (command, allow_empty);
        Err("SDK query unavailable outside WASM".into())
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn decode_reply<R: Message + Default>(bytes: &[u8]) -> Result<R, String> {
    if bytes.is_empty() {
        return Err("Empty SDK reply".into());
    }
    R::decode(bytes).map_err(|error| format!("Malformed SDK reply: {error}"))
}

pub fn focused_pane() -> Result<(usize, PaneId), String> {
    let response: ProtobufGetFocusedPaneInfoResponse = query(PluginCommand::GetFocusedPaneInfo)?;
    match response.result {
        Some(get_focused_pane_info_response::Result::FocusedPaneInfo(info)) => {
            let pane = info
                .focused_pane_id
                .ok_or("Missing focused pane ID")?
                .try_into()
                .map_err(|_| "Invalid focused pane ID")?;
            Ok((info.focused_tab_index as usize, pane))
        }
        Some(get_focused_pane_info_response::Result::Error(error)) => Err(error),
        None => Err("Missing focused pane result".into()),
    }
}

pub fn pane_info(pane: PaneId) -> Option<PaneInfo> {
    live_pane_info(pane).ok().flatten()
}

pub fn live_pane_info(pane: PaneId) -> Result<Option<PaneInfo>, String> {
    let response: ProtobufGetPaneInfoResponse =
        query_with_empty(PluginCommand::GetPaneInfo(pane), true)?;
    response.pane_info
        .map(|info| info.try_into().map_err(|error| format!("Malformed pane info: {error}")))
        .transpose()
}

#[cfg(target_arch = "wasm32")]
pub fn tab_info(tab_id: usize) -> Result<Option<TabInfo>, String> {
    let response: ProtobufGetTabInfoResponse =
        query_with_empty(PluginCommand::GetTabInfo(tab_id), true)?;
    response.tab_info
        .map(|info| info.try_into().map_err(|error| format!("Malformed tab info: {error}")))
        .transpose()
}

#[cfg(target_arch = "wasm32")]
pub fn session_list() -> Result<SessionListSnapshot, String> {
    let response: ProtobufGetSessionListResponse = query(PluginCommand::GetSessionList)?;
    match response.result {
        Some(get_session_list_response::Result::Snapshot(snapshot)) => {
            let live_sessions = snapshot
                .live_sessions
                .into_iter()
                .map(SessionInfo::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Malformed session manifest: {error}"))?;
            Ok(SessionListSnapshot {
                live_sessions,
                resurrectable_sessions: snapshot
                    .resurrectable_sessions
                    .into_iter()
                    .map(<(String, std::time::Duration)>::from)
                    .collect(),
            })
        }
        Some(get_session_list_response::Result::Error(error)) => Err(error),
        None => Err("Missing session list result".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_malformed_replies_fail_without_trapping() {
        assert!(decode_reply::<ProtobufGetFocusedPaneInfoResponse>(&[]).is_err());
        assert!(decode_reply::<ProtobufGetFocusedPaneInfoResponse>(&[0xff]).is_err());
        let denied = ProtobufGetFocusedPaneInfoResponse {
            result: Some(get_focused_pane_info_response::Result::Error(
                "denied".into(),
            )),
        };
        let decoded: ProtobufGetFocusedPaneInfoResponse =
            decode_reply(&denied.encode_to_vec()).unwrap();
        assert_eq!(decoded, denied);
    }
}
