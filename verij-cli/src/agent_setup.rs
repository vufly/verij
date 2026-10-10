//! Preliminary H3 setup: embedded assets and a surgical TUI config edit.
use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use serde_json::Value;
use std::path::{Path, PathBuf};

const BRIDGE: &str = include_str!("../../integrations/opencode/bridge.mjs");
const CORE: &str = include_str!("../../integrations/opencode/core.mjs");
const ID: &str = "verij.monitoring.v1";

#[derive(Debug, Clone, ValueEnum)]
pub enum Adapter {
    Opencode,
    Agy,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    pub adapter: Adapter,
    /// Adapter configuration directory (OpenCode TUI config or Agy .gemini).
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
    /// OpenCode binary used for version detection.
    #[arg(long, default_value = "opencode")]
    pub opencode_bin: PathBuf,
    #[arg(long, default_value = "agy")]
    pub agy_bin: PathBuf,
    #[arg(long, default_value = "python3")]
    pub python_bin: PathBuf,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(value_enum, default_value = "opencode")]
    pub adapter: Adapter,
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
    #[arg(long, default_value = "opencode")]
    pub opencode_bin: PathBuf,
    #[arg(long, default_value = "agy")]
    pub agy_bin: PathBuf,
    /// Diagnose the exporter prerequisite for one inner Zellij session.
    #[arg(long)]
    pub session: Option<String>,
}

fn directory(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    let path = override_dir
        .or_else(|| std::env::var_os("OPENCODE_CONFIG_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(|p| PathBuf::from(p).join("opencode")))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/opencode")))
        .context("OpenCode configuration directory unavailable")?;
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn config_path(directory: &Path) -> Result<PathBuf> {
    let json = directory.join("tui.json");
    let jsonc = directory.join("tui.jsonc");
    if json.exists() && jsonc.exists() {
        bail!("both tui.json and tui.jsonc exist; select one config before setup");
    }
    Ok(if jsonc.exists() { jsonc } else { json })
}

fn target(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    if override_dir.is_none() {
        if let Some(file) = std::env::var_os("OPENCODE_TUI_CONFIG") {
            let path = PathBuf::from(file);
            return Ok(if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path)
            });
        }
    }
    config_path(&directory(override_dir)?)
}

// Stable content fingerprint for accidental asset-edit detection (not an
// authentication mechanism). Manifest allows future embedded bridge upgrades.
pub(crate) fn fingerprint(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn asset_manifest(assets: &Path) -> Result<Value> {
    let file = assets.join("installed.json");
    if file.exists() {
        Ok(serde_json::from_slice(&std::fs::read(file)?)?)
    } else {
        Ok(serde_json::json!({}))
    }
}

fn validate_assets(assets: &Path, manifest: &Value) -> Result<()> {
    for (name, expected) in [("bridge.mjs", BRIDGE), ("core.mjs", CORE)] {
        let file = assets.join(name);
        if file.exists() {
            let existing = std::fs::read_to_string(&file)?;
            if existing != expected
                && manifest[name].as_str() != Some(fingerprint(&existing).as_str())
            {
                bail!(
                    "modified adapter asset {}; preserve edits before setup",
                    file.display()
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn version(binary: &Path) -> Result<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if !output.status.success() {
                bail!("OpenCode version detection failed");
            }
            return Ok(String::from_utf8(output.stdout)?.trim().to_string());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("OpenCode version detection timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Mask JSONC comments and trailing commas while retaining byte offsets.
fn mask_comments(text: &str) -> Result<Vec<u8>> {
    let mut bytes = text.as_bytes().to_vec();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                bytes[i] = b' ';
                i += 1;
            }
            if i + 1 >= bytes.len() {
                bail!("unterminated JSONC comment");
            }
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
        } else if bytes[i] == b',' {
            // Comments may follow a trailing comma; remove after masking below.
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(bytes)
}

fn mask(text: &str) -> Result<String> {
    let mut bytes = mask_comments(text)?;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i = string_end(&bytes, i);
            continue;
        }
        if bytes[i] == b',' {
            let next = bytes[i + 1..]
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .map(|n| n + i + 1);
            if next.is_some_and(|n| matches!(bytes[n], b']' | b'}')) {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }
    Ok(String::from_utf8(bytes)?)
}

fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return i + 1;
        } else {
            i += 1;
        }
    }
    bytes.len()
}

fn value_end(bytes: &[u8], start: usize) -> usize {
    if bytes[start] == b'"' {
        return string_end(bytes, start);
    }
    let mut depth = 0;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i = string_end(bytes, i);
            continue;
        }
        match bytes[i] {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b',' if depth == 0 => return i,
            _ => (),
        }
        i += 1;
    }
    i
}

fn plugin_span(masked: &str) -> Option<(usize, usize)> {
    let bytes = masked.as_bytes();
    let mut i = masked.find('{')? + 1;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if bytes.get(i) != Some(&b'"') {
            return None;
        }
        let end = string_end(bytes, i);
        let key: String = serde_json::from_slice(&bytes[i..end]).ok()?;
        i = end;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b':')
        {
            i += 1;
        }
        let end = value_end(bytes, i);
        if key == "plugin" {
            return Some((i, end));
        }
        i = end;
    }
    None
}

fn owned(value: &Value, asset: &str) -> bool {
    value.as_str() == Some(asset)
        || value
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            == Some(asset)
}

/// Surgical entry edits preserve unrelated plugin options and JSONC comments.
fn edit(text: &str, asset: &str, entry: Option<Value>) -> Result<String> {
    let masked = mask(text)?;
    let mut config: Value =
        serde_json::from_str(&masked).context("invalid OpenCode TUI JSON/JSONC")?;
    let object = config
        .as_object_mut()
        .context("TUI config must be an object")?;
    let mut plugins = object
        .get("plugin")
        .map(|v| v.as_array().cloned().context("plugin must be an array"))
        .transpose()?
        .unwrap_or_default();
    let existing: Vec<_> = plugins.iter().filter(|p| owned(p, asset)).collect();
    if existing.len() == 1 && entry.as_ref() == existing.first().copied() {
        return Ok(text.to_string());
    }
    if let Some((start, end)) = plugin_span(&masked) {
        let comments = mask_comments(text)?;
        let mut spans = Vec::new();
        let mut offset = start + 1;
        while offset < end - 1 {
            while offset < end - 1
                && (comments[offset].is_ascii_whitespace() || comments[offset] == b',')
            {
                offset += 1;
            }
            if offset >= end - 1 {
                break;
            }
            let next = value_end(&comments, offset);
            spans.push((offset, next));
            offset = next;
        }
        if spans.len() != plugins.len() {
            bail!("ambiguous plugin array");
        }
        let removed: Vec<_> = plugins.iter().map(|v| owned(v, asset)).collect();
        let retained = removed.iter().filter(|r| !**r).count();
        let mut ranges = Vec::new();
        for (index, &(from, to)) in spans.iter().enumerate() {
            if !removed[index] {
                continue;
            }
            ranges.push((from, to));
            let has_next_retained = removed[index + 1..].iter().any(|r| !*r);
            let (left, right) = if has_next_retained {
                (to, spans.get(index + 1).map_or(end - 1, |s| s.0))
            } else {
                (
                    if index == 0 {
                        start + 1
                    } else {
                        spans[index - 1].1
                    },
                    from,
                )
            };
            if let Some(comma) = comments[left..right].iter().position(|b| *b == b',') {
                ranges.push((left + comma, left + comma + 1));
            }
        }
        if retained == 0 {
            // Remove any original trailing separator too, retaining comments.
            for (index, byte) in comments.iter().enumerate().take(end - 1).skip(start + 1) {
                if *byte == b',' && !spans.iter().any(|(a, b)| index >= *a && index < *b) {
                    ranges.push((index, index + 1));
                }
            }
        }
        ranges.sort_unstable();
        ranges.dedup();
        let mut output = text.to_string();
        if let Some(entry) = entry {
            let trailing = comments[start + 1..end - 1]
                .iter()
                .rfind(|b| !b.is_ascii_whitespace())
                == Some(&b',');
            let comma = if retained == 0 || trailing { "" } else { "," };
            output.insert_str(
                end - 1,
                &format!("{comma}\n    {}\n  ", serde_json::to_string(&entry)?),
            );
        }
        for (from, to) in ranges.into_iter().rev() {
            output.replace_range(from..to, "");
        }
        // Ensure surgery never publishes malformed configuration.
        serde_json::from_str::<Value>(&mask(&output)?)?;
        Ok(output)
    } else if let Some(entry) = entry {
        plugins.push(entry);
        let array = serde_json::to_string_pretty(&plugins)?;
        let end = masked.rfind('}').context("missing object end")?;
        let prefix = masked[..end].trim_end();
        // A masked trailing comma is blank; inspect its original byte at the
        // first offset after the last non-comment/non-whitespace token.
        let has_trailing_comma = text[prefix.len()..end].trim_start().starts_with(',');
        let comma = if object.is_empty() || prefix.ends_with(',') || has_trailing_comma {
            ""
        } else {
            ","
        };
        Ok(format!(
            "{}{}\n  \"plugin\": {}\n{}",
            &text[..end],
            comma,
            array,
            &text[end..]
        ))
    } else {
        Ok(text.to_string())
    }
}

fn atomic_text(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

pub fn setup(args: SetupArgs) -> Result<()> {
    if matches!(args.adapter, Adapter::Agy) { return crate::agy_setup::setup(args); }
    let version = version(&args.opencode_bin)?;
    if !crate::opencode::SUPPORTED.contains(&version.as_str()) {
        bail!(
            "unsupported OpenCode {version}; verified: {:?}",
            crate::opencode::SUPPORTED
        );
    }
    let path = target(args.config_dir)?;
    let dir = path.parent().context("TUI config has no parent")?;
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    let text = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        "{\n  \"$schema\": \"https://opencode.ai/tui.json\"\n}\n".into()
    };
    let assets = dir.join("verij-monitoring");
    let bridge = assets.join("bridge.mjs");
    let reporter = std::env::current_exe()?;
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let states = crate::fs_watcher::resolve_states_dir();
    let entry =
        serde_json::json!([bridge, {"reporter":reporter,"runtimeDir":runtime,"statesDir":states}]);
    let edited = edit(
        &text,
        bridge.to_str().context("non-UTF8 plugin path")?,
        Some(entry),
    )?;
    std::fs::create_dir_all(&assets)?;
    let manifest = asset_manifest(&assets)?;
    validate_assets(&assets, &manifest)?;
    atomic_text(&assets.join("core.mjs"), CORE)?;
    atomic_text(&bridge, BRIDGE)?;
    crate::agent_store::atomic_write_json(
        &assets.join("installed.json"),
        &serde_json::json!({
            "bridge.mjs":fingerprint(BRIDGE), "core.mjs":fingerprint(CORE)
        }),
    )?;
    if edited != text {
        atomic_text(&path, &edited)?;
    }
    println!(
        "{}",
        serde_json::json!({"installed":true,"bridge_version":crate::opencode::BRIDGE_VERSION,
        "opencode_version":version,"config":path,"reporter":reporter,"runtime":runtime,"restart_required":true})
    );
    Ok(())
}

pub fn uninstall(args: SetupArgs) -> Result<()> {
    if matches!(args.adapter, Adapter::Agy) { return crate::agy_setup::uninstall(args); }
    let path = target(args.config_dir)?;
    let dir = path.parent().context("TUI config has no parent")?;
    if !dir.exists() {
        println!("{{\"uninstalled\":true}}");
        return Ok(());
    }
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    let assets = dir.join("verij-monitoring");
    let bridge = assets.join("bridge.mjs");
    let manifest = asset_manifest(&assets)?;
    if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        let edited = edit(
            &text,
            bridge.to_str().context("non-UTF8 plugin path")?,
            None,
        )?;
        if edited != text {
            atomic_text(&path, &edited)?;
        }
    }
    // Modified assets may be user work. Remove only exact embedded content.
    for (file, expected) in [(bridge, BRIDGE), (assets.join("core.mjs"), CORE)] {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if std::fs::read_to_string(&file).is_ok_and(|text| {
            text == expected || manifest[name].as_str() == Some(fingerprint(&text).as_str())
        }) {
            std::fs::remove_file(file)?;
        }
    }
    if assets.join("installed.json").exists() {
        std::fs::remove_file(assets.join("installed.json"))?;
    }
    let _ = std::fs::remove_dir(assets);
    println!("{{\"uninstalled\":true,\"restart_required\":true}}");
    Ok(())
}

pub(crate) fn inventory_diagnostics(snapshots: &[verij_types::SessionSnapshot]) -> Vec<Value> {
    snapshots.iter().map(|snapshot| {
        let (status, detail, terminals) = match snapshot.inventory.as_ref() {
            None => ("legacy_snapshot", "This session exports only sessions/tabs. Upgrade the Verij WASM plugin at its configured load_plugins path, then reload that same URL in this session.", 0),
            Some(inventory) if inventory.schema_version != 1 => (
                "unsupported_schema", "Exporter inventory schema is unsupported by this CLI.", 0,
            ),
            Some(inventory) if inventory.server_process.is_none() => (
                "server_unverified", "Exporter server incarnation is unavailable; refresh the session's Verij exporter.", 0,
            ),
            Some(inventory) => {
                let terminals = inventory.panes.iter().filter(|pane| !pane.exited && pane.pane_process.is_some()).count();
                if terminals == 0 {
                    ("pane_identity_unavailable", "Exporter is present but has no verified live terminal process identity.", 0)
                } else {
                    ("ready", "Verified native pane inventory is available. A foreground OpenCode TUI can register against it.", terminals)
                }
            }
        };
        serde_json::json!({"session":snapshot.name,"status":status,"detail":detail,"verified_terminal_panes":terminals})
    }).collect()
}

pub fn doctor(args: DoctorArgs) -> Result<()> {
    if matches!(args.adapter, Adapter::Agy) { return crate::agy_setup::doctor(args); }
    let path = target(args.config_dir)?;
    let dir = path.parent().context("TUI config has no parent")?;
    let assets = dir.join("verij-monitoring");
    let version = version(&args.opencode_bin).ok();
    let config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| mask(&text).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let entry = config
        .as_ref()
        .and_then(|c| c.get("plugin"))
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|e| owned(e, assets.join("bridge.mjs").to_str().unwrap_or("")))
        });
    let options = entry.and_then(Value::as_array).and_then(|a| a.get(1));
    let disabled = config
        .as_ref()
        .and_then(|c| c.get("plugin_enabled"))
        .and_then(|p| p.get(ID))
        == Some(&Value::Bool(false));
    let exact_assets = std::fs::read_to_string(assets.join("bridge.mjs"))
        .is_ok_and(|s| s == BRIDGE)
        && std::fs::read_to_string(assets.join("core.mjs")).is_ok_and(|s| s == CORE);
    let reporter = options
        .and_then(|o| o.get("reporter"))
        .and_then(Value::as_str);
    let reporter_version = reporter.and_then(|p| self::version(Path::new(p)).ok());
    let reporter_available = reporter_version
        .as_deref()
        .is_some_and(|v| v.starts_with("verij "));
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let check = runtime.join(format!(".doctor-{}", uuid::Uuid::new_v4()));
    crate::agent_store::atomic_write_json(&check, &serde_json::json!({"self_check":true}))?;
    let runtime_writable = std::fs::read(&check).is_ok();
    std::fs::remove_file(&check)?;
    let mut records = crate::agent_watcher::read_records(&runtime.join("agents/v1"));
    let inventory_directory = crate::fs_watcher::resolve_states_dir();
    let mut snapshots = crate::inventory::read_states(&inventory_directory);
    if let Some(session) = args.session.as_deref() {
        snapshots.retain(|snapshot| snapshot.name == session);
        records.retain(|record| {
            snapshots.iter().any(|snapshot| {
                snapshot
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.session_instance_id.as_ref())
                    == Some(&record.identity.pane_key.session)
            })
        });
    }
    let inventory_sessions = inventory_diagnostics(&snapshots);
    let inventory_ready = inventory_sessions.iter().any(|s| s["status"] == "ready");
    let version_supported = version
        .as_deref()
        .is_some_and(|v| crate::opencode::SUPPORTED.contains(&v));
    let adapter_runtime = std::env::var("VERIJ_AGENT_STATE_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| PathBuf::from(value.trim()))
        .or_else(|| {
            options
                .and_then(|o| o.get("runtimeDir"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
    let adapter_inventory = std::env::var_os("VERIJ_STATES_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            options
                .and_then(|o| o.get("statesDir"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
    let runtime_matches = adapter_runtime.as_deref() == Some(runtime.as_path());
    let inventory_path_matches =
        adapter_inventory.as_deref() == Some(inventory_directory.as_path());
    let installation_ready =
        version_supported && entry.is_some() && !disabled && exact_assets && reporter_available;
    let mut blockers = Vec::new();
    if !installation_ready {
        blockers.push("OpenCode adapter installation/version needs attention.");
    }
    if !inventory_ready {
        blockers.push("No verified pane inventory is available for the selected session scope. Check inventory_sessions; legacy snapshots require a Verij WASM exporter upgrade/reload, not another OpenCode restart.");
    }
    if !runtime_matches {
        blockers.push("Current reporter runtime differs from installed adapter runtime; use the same VERIJ_AGENT_STATE_DIR in agent and sidebar.");
    }
    if !inventory_path_matches {
        blockers.push("Current inventory directory differs from installed adapter path; use the same VERIJ_STATES_DIR in agent and sidebar.");
    }
    let bundled_plugin = crate::layout::resolve_plugin_path(None).ok();
    let default_installed_plugin = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|data| data.join("verij/verij_plugin.wasm"));
    let installed_plugin_matches_bundle = bundled_plugin
        .as_ref()
        .zip(default_installed_plugin.as_ref())
        .and_then(|(source, installed)| {
            std::fs::read(source)
                .ok()
                .zip(std::fs::read(installed).ok())
        })
        .map(|(source, installed)| source == installed);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "adapter":"opencode", "bridge_version":crate::opencode::BRIDGE_VERSION,
            "opencode_version":version, "supported_versions":crate::opencode::SUPPORTED,
            "version_supported":version_supported,
            "config":path, "configured":entry.is_some(), "disabled":disabled,
            "assets_match":exact_assets, "reporter":reporter, "reporter_available":reporter_available,"reporter_version":reporter_version,
            "runtime":runtime, "runtime_writable":runtime_writable, "installed_runtime":options.and_then(|o|o.get("runtimeDir")),
            "effective_adapter_runtime":adapter_runtime,"runtime_matches_adapter":runtime_matches,
            "inventory_directory":inventory_directory,"installed_inventory_directory":options.and_then(|o|o.get("statesDir")),
            "effective_adapter_inventory_directory":adapter_inventory,"inventory_path_matches_adapter":inventory_path_matches,
            "requested_session":args.session,"inventory_sessions":inventory_sessions,"inventory_ready":inventory_ready,
            "bundled_plugin":bundled_plugin,"default_installed_plugin":default_installed_plugin,
            "installed_plugin_matches_bundle":installed_plugin_matches_bundle,
            "exporter_upgrade":"From the checkout: make install-plugin (or set VERIJ_DATA_DIR to the directory of your configured Verij WASM). Reload the same configured plugin URL in the affected existing session using zellij -s SESSION action start-or-reload-plugin URL. New sessions load the installed artifact normally.",
            "installation_ready":installation_ready,
            "monitoring_prerequisites_ready":installation_ready && inventory_ready && runtime_matches && inventory_path_matches,
            "blockers":blockers,
            "live_records":records.iter().filter(|r| r.identity.kind == verij_types::agent::AgentKind::Opencode).count(),
            "records":records.iter().filter(|r| r.identity.kind == verij_types::agent::AgentKind::Opencode).map(|r| serde_json::json!({
                "instance":r.identity.agent_instance_id,"pid":r.identity.process.pid,"pane":r.identity.pane_key,
                "status":r.state.reduced.status,"source_healthy":!crate::opencode::lease_expired(&r.state, crate::agent_store::now_ms())
            })).collect::<Vec<_>>(),
            "binding_requires":"Linux foreground tty + current native Zellij inventory",
            "capabilities":["activity","permissions","questions","conversation_title","aggregate_completion","terminal_failure"],
            "note":"Installation, runtime and pane-inventory prerequisite checks. live_records confirms actual reporting; installation alone is insufficient. Restart OpenCode after adapter setup/uninstall; reload the Verij exporter after a WASM upgrade."
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_distinguishes_legacy_exporter_from_verified_pane_inventory() {
        let mut snapshot: verij_types::SessionSnapshot =
            serde_json::from_value(serde_json::json!({
                "name":"legacy-inner","is_current":true,"tabs":[],"active_pane":null
            }))
            .unwrap();
        let legacy = inventory_diagnostics(&[snapshot.clone()]);
        assert_eq!(legacy[0]["status"], "legacy_snapshot");
        assert!(legacy[0]["detail"]
            .as_str()
            .unwrap()
            .contains("reload that same URL"));
        let process = crate::process::identity(std::process::id()).unwrap();
        snapshot.inventory = Some(serde_json::from_value(serde_json::json!({
            "schema_version":1,"producer":{"server_pid":process.pid,"plugin_id":0,"client_id":1,"epoch":"test"},
            "revision":1,"exported_at_ms":crate::agent_store::now_ms(),"tabs":[],"panes":[],"capabilities":[],
            "server_process":process
        })).unwrap());
        assert_eq!(
            inventory_diagnostics(&[snapshot.clone()])[0]["status"],
            "pane_identity_unavailable"
        );
        snapshot.inventory.as_mut().unwrap().panes.push(serde_json::from_value(serde_json::json!({
            "terminal_id":1,"tab_position":0,"title":"OpenCode","pane_pid":process.pid,"pane_process":process,
            "is_floating":false,"is_suppressed":false,"is_fullscreen":false,"layer_focused":false,"exited":false
        })).unwrap());
        assert_eq!(inventory_diagnostics(&[snapshot])[0]["status"], "ready");
    }
    #[test]
    fn jsonc_setup_is_idempotent_and_uninstall_preserves_other_settings() {
        let original = "{\n // user comment\n \"theme\":\"mine\",\n \"plugin\":[[\"other\",{\"x\":1}],],\n \"keybinds\":{\"leader\":\"ctrl+x\"},\n}";
        let entry = serde_json::json!(["/owned", {"reporter":"/verij"}]);
        let installed = edit(original, "/owned", Some(entry.clone())).unwrap();
        assert!(installed.contains("// user comment"));
        // No rewrite when exactly one existing entry already has these options.
        assert_eq!(edit(&installed, "/owned", Some(entry)).unwrap(), installed);
        let removed = edit(&installed, "/owned", None).unwrap();
        let value: Value = serde_json::from_str(&mask(&removed).unwrap()).unwrap();
        assert_eq!(value["plugin"], serde_json::json!([["other", {"x":1}]]));
        assert_eq!(value["theme"], "mine");
        assert_eq!(value["keybinds"]["leader"], "ctrl+x");
        assert!(edit("{broken", "/owned", None).is_err());
    }
    #[test]
    fn adding_first_plugin_handles_jsonc_trailing_comma() {
        for input in ["{}", "{\"theme\":\"x\"}", "{\"theme\":\"x\", /* note */}"] {
            let output = edit(input, "/owned", Some(serde_json::json!("/owned"))).unwrap();
            assert!(
                serde_json::from_str::<Value>(&mask(&output).unwrap()).is_ok(),
                "{output}"
            );
        }
    }

    #[test]
    fn plugin_surgery_preserves_comments_and_removes_duplicate_owned_entries() {
        for input in [
            "{\"plugin\":[/* keep */ \"other\", \"/owned\", \"/owned\"]}",
            "{\"plugin\":[\"/owned\", /* keep */ \"other\", \"/owned\",]}",
            "{\"plugin\":[\"/owned\", /* keep */ \"/owned\",]}",
        ] {
            let result = edit(input, "/owned", None).unwrap();
            assert!(result.contains("/* keep */"));
            let parsed: Value = serde_json::from_str(&mask(&result).unwrap()).unwrap();
            assert!(!parsed["plugin"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| owned(p, "/owned")));
        }
    }

    #[test]
    fn updating_options_and_modified_assets_preserve_user_work() {
        let original = "{\"plugin\":[/* own */[\"/owned\",{\"reporter\":\"old\"}],/* keep */[\"other\",{\"x\":1}],]}";
        let edited = edit(
            original,
            "/owned",
            Some(serde_json::json!(["/owned", {"reporter":"new"}])),
        )
        .unwrap();
        assert!(edited.contains("/* keep */[\"other\",{\"x\":1}]"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("core.mjs"), "user edits").unwrap();
        assert!(validate_assets(dir.path(), &serde_json::json!({})).is_err());
        assert!(validate_assets(
            dir.path(),
            &serde_json::json!({"core.mjs":fingerprint("user edits")})
        )
        .is_ok());
    }
}
