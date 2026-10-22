//! Explicitly opted-in OpenCode V2 live hints. No full-recovery admission.
use crate::agent_setup::{
    atomic_text, directory, edit_plugins, fingerprint, mask, owned, DoctorArgs, SetupArgs,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const ID: &str = "verij.monitoring.v2.live";
const ASSETS: [(&str, &str); 3] = [
    (
        "tui.js",
        include_str!("../../integrations/opencode/v2/tui.js"),
    ),
    (
        "core.mjs",
        include_str!("../../integrations/opencode/v2/core.mjs"),
    ),
    (
        "reporter.mjs",
        include_str!("../../integrations/opencode/v2/reporter.mjs"),
    ),
];
const DIR: &str = "verij-monitoring-v2-live";
const LIMITATIONS: &str = "Experimental cache/event hints only. No Done, terminal Error, outcome recovery, complete-family freshness, pre-parse upstream bounds, or authenticated-remote qualification. Selected quiescent state is Unknown. OpenCode owns cache freshness and allocation.";

pub fn installed(config_dir: Option<PathBuf>) -> Result<bool> {
    Ok(directory(config_dir)?
        .join(DIR)
        .join("installed.json")
        .exists())
}

fn read_config(path: &Path) -> Result<String> {
    Ok(if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        "{}\n".into()
    })
}
fn parse(text: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(&mask(text)?)?;
    if !value.is_object() {
        bail!("OpenCode CLI config must be an object");
    }
    Ok(value)
}
fn manifest(assets: &Path) -> Result<Value> {
    let path = assets.join("installed.json");
    Ok(if path.exists() {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        json!({})
    })
}
fn inline_override() -> bool {
    std::env::var("OPENCODE_CLI_CONFIG_CONTENT")
        .is_ok_and(|text| parse(&text).map_or(true, |config| config.get("plugins").is_some()))
}
fn disabled(config: &Value, package: &str) -> bool {
    config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_str()
                    .and_then(|s| s.strip_prefix('-'))
                    .is_some_and(|selector| {
                        selector == ID || selector == package || selector.contains('*')
                    })
                    || (owned(entry, package) && entry.get("enabled") == Some(&Value::Bool(false)))
            })
        })
}
fn migration_pending(dir: &Path) -> bool {
    !dir.join("cli.json").exists()
        && ["tui.json", "tui.jsonc", "kv.json"]
            .iter()
            .any(|name| dir.join(name).exists())
}

pub fn setup(args: SetupArgs, version: &str) -> Result<()> {
    if !crate::opencode::supports_live_version(version) {
        bail!("experimental live status requires OpenCode 2.x >= 2.0.32");
    }
    let dir = directory(args.config_dir)?;
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    if migration_pending(&dir) {
        bail!("OpenCode V1 preferences need first-start migration/review: start the V2 CLI once in this config environment, then rerun setup; tui.jsonc may require manual review");
    }
    if inline_override() {
        bail!(
            "OPENCODE_CLI_CONFIG_CONTENT overrides plugins or is invalid; resolve it before setup"
        );
    }
    let path = dir.join("cli.json");
    let assets = dir.join(DIR);
    let package = assets.to_str().context("non-UTF8 plugin path")?;
    let text = read_config(&path)?;
    let config = parse(&text)?;
    if disabled(&config, package) {
        bail!("OpenCode CLI plugin is disabled or a wildcard disable directive needs review; preserving user choice");
    }
    let previous = manifest(&assets)?;
    for (name, embedded) in ASSETS {
        let file = assets.join(name);
        if file.exists() {
            let existing = std::fs::read_to_string(&file)?;
            if existing != embedded
                && previous.get(name).and_then(Value::as_str)
                    != Some(fingerprint(&existing).as_str())
            {
                bail!("modified/unowned V2 asset preserved: {}", file.display());
            }
        }
    }
    let reporter = std::env::current_exe()?;
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let states = crate::fs_watcher::resolve_states_dir();
    let mut base = text.clone();
    let legacy = dir.join("verij-monitoring/bridge.mjs");
    let legacy_path = legacy.to_str().context("non-UTF8 legacy path")?;
    if config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|entries| entries.iter().any(|e| owned(e, legacy_path)))
    {
        let old_manifest = manifest(&dir.join("verij-monitoring"))?;
        let content = std::fs::read_to_string(&legacy)
            .context("migrated V1 entry has no verifiable owned asset; review cli.json")?;
        if old_manifest["bridge.mjs"].as_str() != Some(fingerprint(&content).as_str()) {
            bail!("migrated V1 entry ownership ambiguous; preserving cli.json");
        }
        base = edit_plugins(&base, "plugins", legacy_path, None)?;
    }
    let entry = json!({"package":package,"options":{"reporter":reporter,"runtimeDir":runtime,
        "statesDir":states,"experimentalLiveStatus":true}});
    let edited = edit_plugins(&base, "plugins", package, Some(entry))?;
    std::fs::create_dir_all(&assets)?;
    let mut next =
        json!({"adapter":ID,"schema_version":2,"config":path,"opencode_version":version});
    for (name, embedded) in ASSETS {
        atomic_text(&assets.join(name), embedded)?;
        next[name] = json!(fingerprint(embedded));
    }
    crate::agent_store::atomic_write_json(&assets.join("installed.json"), &next)?;
    if edited != text || !path.exists() {
        atomic_text(&path, &edited)?;
    }
    println!(
        "{}",
        json!({"installed":true,"experimental":true,"mode":"live_status",
        "bridge_version":2,"opencode_version":version,"config":path,"reporter":reporter,
        "runtime":runtime,"restart_required":true,"limitations":LIMITATIONS})
    );
    Ok(())
}

pub fn uninstall(args: SetupArgs) -> Result<()> {
    // Deliberately independent of the selected executable/version.
    let dir = directory(args.config_dir)?;
    if !dir.exists() {
        println!("{{\"uninstalled\":true}}");
        return Ok(());
    }
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    let assets = dir.join(DIR);
    let path = dir.join("cli.json");
    let previous = manifest(&assets)?;
    if path.exists() {
        let text = read_config(&path)?;
        let edited = edit_plugins(
            &text,
            "plugins",
            assets.to_str().context("non-UTF8 plugin path")?,
            None,
        )?;
        if edited != text {
            atomic_text(&path, &edited)?;
        }
    }
    let mut preserved = Vec::new();
    for (name, embedded) in ASSETS {
        let file = assets.join(name);
        if !file.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&file)?;
        if text == embedded || previous[name].as_str() == Some(fingerprint(&text).as_str()) {
            std::fs::remove_file(file)?;
        } else {
            preserved.push(file);
        }
    }
    if assets.join("installed.json").exists() {
        std::fs::remove_file(assets.join("installed.json"))?;
    }
    let _ = std::fs::remove_dir(assets);
    println!(
        "{}",
        json!({"uninstalled":true,"mode":"live_status","restart_required":true,"preserved":preserved})
    );
    Ok(())
}

pub fn doctor(args: DoctorArgs, version: Option<String>) -> Result<()> {
    let dir = directory(args.config_dir)?;
    let path = dir.join("cli.json");
    let assets = dir.join(DIR);
    let config = read_config(&path).and_then(|text| parse(&text));
    let package = assets.to_str().context("non-UTF8 plugin path")?;
    let entry = config
        .as_ref()
        .ok()
        .and_then(|c| c.get("plugins"))
        .and_then(Value::as_array)
        .and_then(|entries| entries.iter().find(|e| owned(e, package)));
    let options = entry.and_then(|e| e.get("options"));
    let opted_in = options.is_some_and(|o| o["experimentalLiveStatus"] == true);
    let exact_assets = ASSETS
        .iter()
        .all(|(name, value)| std::fs::read_to_string(assets.join(name)).is_ok_and(|s| s == *value));
    let reporter = options.and_then(|o| o["reporter"].as_str());
    let reporter_available = reporter
        .and_then(|p| crate::agent_setup::version(Path::new(p)).ok())
        .is_some_and(|s| s.starts_with("verij "));
    let version_supported = version
        .as_deref()
        .is_some_and(crate::opencode::supports_live_version);
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let inventory_dir = crate::fs_watcher::resolve_states_dir();
    let adapter_runtime = std::env::var_os("VERIJ_AGENT_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            options
                .and_then(|o| o["runtimeDir"].as_str())
                .map(PathBuf::from)
        });
    let adapter_inventory = std::env::var_os("VERIJ_STATES_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            options
                .and_then(|o| o["statesDir"].as_str())
                .map(PathBuf::from)
        });
    let runtime_matches = adapter_runtime.as_deref() == Some(runtime.as_path());
    let inventory_matches = adapter_inventory.as_deref() == Some(inventory_dir.as_path());
    let mut snapshots = crate::inventory::read_states(&inventory_dir);
    if let Some(session) = args.session.as_deref() {
        snapshots.retain(|s| s.name == session);
    }
    let inventory = crate::agent_setup::inventory_diagnostics(&snapshots);
    let inventory_ready = inventory.iter().any(|s| s["status"] == "ready");
    let is_disabled = config.as_ref().is_ok_and(|c| disabled(c, package));
    let overridden = inline_override();
    let installation_ready = config.is_ok()
        && version_supported
        && entry.is_some()
        && opted_in
        && exact_assets
        && reporter_available
        && !is_disabled
        && !overridden;
    let records = crate::agent_watcher::read_records(&runtime.join("agents/v1"));
    let live: Vec<_> = records.iter().filter(|r| r.identity.kind == verij_types::agent::AgentKind::Opencode)
        .filter(|r| args.session.is_none() || snapshots.iter().any(|s| s.inventory.as_ref().and_then(|i| i.session_instance_id.as_ref()) == Some(&r.identity.pane_key.session)))
        .map(|r| json!({"instance":r.identity.agent_instance_id,"status":r.state.reduced.status,
            "experimental_live_status":r.state.sources.get("opencode").is_some_and(|s| s.capabilities.iter().any(|c| c == "experimental_live_status")),
            "source_healthy":r.state.sources.contains_key("opencode") && !crate::opencode::lease_expired(&r.state, crate::agent_store::now_ms())})).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"adapter":"opencode","mode":"live_status","experimental":true,
        "bridge_version":2,"opencode_version":version,"supported_versions":[crate::opencode::LIVE_SUPPORTED_VERSIONS],
        "version_supported":version_supported,"config":path,"config_valid":config.is_ok(),"configured":entry.is_some(),
        "opted_in":opted_in,"disabled":is_disabled,"inline_plugins_override":overridden,"migration_pending":migration_pending(&dir),
        "assets_match":exact_assets,"reporter":reporter,"reporter_available":reporter_available,
        "runtime":runtime,"runtime_matches_adapter":runtime_matches,"inventory_directory":inventory_dir,
        "inventory_path_matches_adapter":inventory_matches,"inventory_sessions":inventory,"inventory_ready":inventory_ready,
        "installation_ready":installation_ready,"monitoring_prerequisites_ready":installation_ready && inventory_ready && runtime_matches && inventory_matches,
        "live_records":live.len(),"records":live,"capabilities":["activity","permissions","forms","experimental_live_status","lease"],
        "limitations":LIMITATIONS,"note":"Installation is not runtime qualification. Restart the intended CLI after setup/uninstall. V1 installation is separate; Mini/headless are unsupported."})
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_jsonc_surgery_preserves_directives_and_other_plugin_options() {
        let text = "{\n// keep\n\"theme\":\"mine\",\"plugins\":[{\"package\":\"other\",\"options\":{\"x\":1}},\"-unrelated\",],}";
        let entry = json!({"package":"/owned","options":{"experimentalLiveStatus":true}});
        let installed = edit_plugins(text, "plugins", "/owned", Some(entry.clone())).unwrap();
        assert_eq!(
            edit_plugins(&installed, "plugins", "/owned", Some(entry)).unwrap(),
            installed
        );
        let removed = edit_plugins(&installed, "plugins", "/owned", None).unwrap();
        assert_eq!(parse(&removed).unwrap(), parse(text).unwrap());
        assert!(removed.contains("// keep"));
        assert!(!disabled(&parse(text).unwrap(), "/owned"));
        for directive in ["-*", "-verij.monitoring.v2.live", "-/owned"] {
            assert!(disabled(&json!({"plugins":[directive]}), "/owned"));
        }
    }
}
