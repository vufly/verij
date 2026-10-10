//! Owned Agy callback composition and neutral lifecycle installation.
use crate::agent_setup::{DoctorArgs, SetupArgs};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const CALLBACK: &str = include_str!("../../integrations/agy/callback.py");
const OWNER: &str = "verij-monitoring-v1";

fn directory(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    let path = override_dir
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".gemini")))
        .context("Agy configuration directory unavailable")?;
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn read(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    if !value.is_object() {
        bail!("Agy config must be an object: {}", path.display());
    }
    Ok(value)
}

fn write(path: &Path, value: &Value) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("config parent unavailable")?)?;
    crate::agent_store::atomic_write_json(path, value)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn python_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if path.components().count() > 1 {
        return Ok(std::env::current_dir()?.join(path));
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(path))
        .find(|p| p.is_file())
        .context("Python executable unavailable")
}

fn commands(assets: &Path, python: &Path) -> Value {
    let base = format!(
        "{} {}",
        quote(&python.to_string_lossy()),
        quote(&assets.join("callback.py").to_string_lossy())
    );
    let config = quote(&assets.join("adapter.json").to_string_lossy());
    json!({"ui":format!("{base} ui {config}"),
        "PreInvocation":format!("{base} PreInvocation {config}"),
        "PostInvocation":format!("{base} PostInvocation {config}"),
        "Stop":format!("{base} Stop {config}")})
}

fn hooks(commands: &Value) -> Value {
    let mut hooks = json!({});
    for event in ["PreInvocation", "PostInvocation", "Stop"] {
        hooks[event] = json!([{"type":"command", "command":commands[event], "timeout":2}]);
    }
    hooks
}

fn effective_assets(dir: &Path, status: &Value) -> PathBuf {
    let local = dir.join("verij-monitoring-agy");
    if local.join("installed.json").exists() {
        return local;
    }
    status["statusLine"]["command"]
        .as_str()
        .and_then(|command| command.rsplit_once(" ui "))
        .and_then(|(_, path)| path.strip_prefix('\'').and_then(|p| p.strip_suffix('\'')))
        .map(|path| PathBuf::from(path.replace("'\\''", "'")))
        .filter(|path| {
            path.is_absolute() && path.file_name().is_some_and(|name| name == "adapter.json")
        })
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or(local)
}

pub fn setup(args: SetupArgs) -> Result<()> {
    let version = crate::agent_setup::version(&args.agy_bin)?;
    if !crate::agy::SUPPORTED.contains(&version.as_str()) {
        bail!("unsupported Agy {version}");
    }
    let dir = directory(args.config_dir)?;
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    let assets = dir.join("verij-monitoring-agy");
    let manifest_path = assets.join("installed.json");
    let status_path = dir.join("antigravity-cli/settings.json");
    let hooks_path = dir.join("config/hooks.json");
    let mut status = read(&status_path)?;
    let mut hook_config = read(&hooks_path)?;
    let old_assets = effective_assets(&dir, &status);
    let old = read(&old_assets.join("installed.json"))?;
    let previous = if old.get("commands").is_some()
        && (old["uninstalled"] != true || status.get("statusLine") == old.get("installed_status"))
    {
        if status.get("statusLine") != old.get("installed_status")
            || hook_config.get(OWNER) != old.get("installed_hooks")
        {
            bail!("Verij Agy entries were edited; preserve those edits before setup");
        }
        old["previous_status"].clone()
    } else {
        if old.get("commands").is_some() && status["statusLine"]["command"] == old["commands"]["ui"]
        {
            bail!("edited Verij passthrough must be restored before setup");
        }
        if hook_config.get(OWNER).is_some() {
            bail!("Agy hook ownership collision");
        }
        status.get("statusLine").cloned().unwrap_or(Value::Null)
    };
    let python = python_path(&args.python_bin)?;
    let commands = commands(&assets, &python);
    let mut installed = if previous.is_object() {
        previous.clone()
    } else {
        json!({})
    };
    installed["type"] = json!("command");
    installed["command"] = commands["ui"].clone();
    // Existing enabled/padding/stack flags retain their semantics. An absent
    // custom command stacks the empty callback with the built-in status line.
    if installed.get("enabled").is_none() {
        installed["enabled"] = json!(true);
    }
    if previous
        .get("command")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        installed["stack_with_default"] = json!(true);
    }
    let installed_hooks = hooks(&commands);
    std::fs::create_dir_all(&assets)?;
    let callback = assets.join("callback.py");
    if callback.exists() {
        let existing = std::fs::read_to_string(&callback)?;
        if existing != CALLBACK
            && old["callback_fingerprint"].as_str()
                != Some(&crate::agent_setup::fingerprint(&existing))
        {
            bail!("modified Agy adapter asset");
        }
    }
    // The asset has no secrets; publish through the same atomic helper.
    let temp = callback.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, CALLBACK)?;
    std::fs::rename(temp, callback)?;
    let reporter = std::env::current_exe()?;
    use std::os::unix::fs::MetadataExt;
    let executable = std::fs::metadata(python_path(&args.agy_bin)?)?;
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let states = crate::fs_watcher::resolve_states_dir();
    write(
        &assets.join("adapter.json"),
        &json!({"enabled":true,"version":version,"reporter":reporter,"runtime":runtime,"states":states,
        "binary_dev":executable.dev(),"binary_ino":executable.ino(),
        "previous_command":previous.get("command").and_then(Value::as_str).filter(|_| previous.get("type").and_then(Value::as_str) == Some("command"))}),
    )?;
    write(
        &manifest_path,
        &json!({"commands":commands,"previous_status":previous,"installed_status":installed,"installed_hooks":installed_hooks,
        "callback_fingerprint":crate::agent_setup::fingerprint(CALLBACK)}),
    )?;
    status["statusLine"] = installed;
    hook_config[OWNER] = installed_hooks;
    write(&hooks_path, &hook_config)?;
    write(&status_path, &status)?;
    println!(
        "{}",
        json!({"installed":true,"adapter":"agy","version":version,"config_dir":dir,
        "reporter":reporter,"runtime":runtime,"restart_required":true})
    );
    Ok(())
}

pub fn uninstall(args: SetupArgs) -> Result<()> {
    let dir = directory(args.config_dir)?;
    if !dir.exists() {
        println!("{{\"uninstalled\":true}}");
        return Ok(());
    }
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;
    let mut status = read(&dir.join("antigravity-cli/settings.json"))?;
    let assets = effective_assets(&dir, &status);
    let mut manifest = read(&assets.join("installed.json"))?;
    let mut hook_config = read(&dir.join("config/hooks.json"))?;
    if manifest.get("commands").is_some() {
        if status.get("statusLine") == manifest.get("installed_status") {
            if manifest["previous_status"].is_null() {
                status.as_object_mut().unwrap().remove("statusLine");
            } else {
                status["statusLine"] = manifest["previous_status"].clone();
            }
            write(&dir.join("antigravity-cli/settings.json"), &status)?;
        }
        if hook_config.get(OWNER) == manifest.get("installed_hooks") {
            hook_config.as_object_mut().unwrap().remove(OWNER);
            write(&dir.join("config/hooks.json"), &hook_config)?;
        }
        // Other Magy homes can retain copied absolute commands. Keep a neutral
        // passthrough asset rather than breaking those callbacks/hooks.
        if assets == dir.join("verij-monitoring-agy") {
            let mut adapter = read(&assets.join("adapter.json"))?;
            adapter["enabled"] = json!(false);
            write(&assets.join("adapter.json"), &adapter)?;
            manifest["uninstalled"] = json!(true);
            write(&assets.join("installed.json"), &manifest)?;
        }
    }
    println!("{{\"uninstalled\":true,\"restart_required\":true}}");
    Ok(())
}

pub fn doctor(args: DoctorArgs) -> Result<()> {
    let dir = directory(args.config_dir)?;
    let status = read(&dir.join("antigravity-cli/settings.json"))?;
    // Magy copies settings/hooks, not adapter assets. A copied absolute command
    // keeps using the original shared assets; diagnose that actual installation.
    let assets = effective_assets(&dir, &status);
    let manifest = read(&assets.join("installed.json"))?;
    let adapter = read(&assets.join("adapter.json"))?;
    let hook_config = read(&dir.join("config/hooks.json"))?;
    let version = crate::agent_setup::version(&args.agy_bin).ok();
    let reporter = adapter["reporter"].as_str();
    let reporter_available = reporter
        .and_then(|p| crate::agent_setup::version(Path::new(p)).ok())
        .is_some_and(|v| v.starts_with("verij "));
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let check = runtime.join(format!(".doctor-{}", uuid::Uuid::new_v4()));
    crate::agent_store::atomic_write_json(&check, &json!({"self_check":true}))?;
    let runtime_writable = std::fs::read(&check).is_ok();
    std::fs::remove_file(check)?;
    let states = crate::fs_watcher::resolve_states_dir();
    let mut snapshots = crate::inventory::read_states(&states);
    if let Some(session) = args.session.as_ref() {
        snapshots.retain(|s| &s.name == session);
    }
    let inventory = crate::agent_setup::inventory_diagnostics(&snapshots);
    let configured = manifest.get("commands").is_some()
        && status.get("statusLine") == manifest.get("installed_status");
    let lifecycle = manifest.get("installed_hooks").is_some()
        && hook_config.get(OWNER) == manifest.get("installed_hooks");
    let exact_assets =
        std::fs::read_to_string(assets.join("callback.py")).is_ok_and(|s| s == CALLBACK);
    use std::os::unix::fs::MetadataExt;
    let binary_matches = python_path(&args.agy_bin)
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|info| {
            adapter["binary_dev"].as_u64() == Some(info.dev())
                && adapter["binary_ino"].as_u64() == Some(info.ino())
        });
    let disabled = status["statusLine"]["enabled"] == false;
    let effective_runtime = std::env::var_os("VERIJ_AGENT_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| adapter["runtime"].as_str().map(PathBuf::from));
    let effective_states = std::env::var_os("VERIJ_STATES_DIR")
        .map(PathBuf::from)
        .or_else(|| adapter["states"].as_str().map(PathBuf::from));
    let ready = configured
        && !disabled
        && adapter["enabled"] != false
        && lifecycle
        && exact_assets
        && reporter_available
        && binary_matches
        && version
            .as_deref()
            .is_some_and(|v| crate::agy::SUPPORTED.contains(&v))
        && effective_runtime.as_deref() == Some(runtime.as_path())
        && effective_states.as_deref() == Some(states.as_path());
    let mut records = crate::agent_watcher::read_records(&runtime.join("agents/v1"));
    records.retain(|r| {
        r.identity.kind == verij_types::agent::AgentKind::Agy
            && (args.session.is_none()
                || snapshots.iter().any(|s| {
                    s.inventory
                        .as_ref()
                        .and_then(|i| i.session_instance_id.as_ref())
                        == Some(&r.identity.pane_key.session)
                }))
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"adapter":"agy","agy_version":version,
        "supported_versions":crate::agy::SUPPORTED,"configured":configured,"lifecycle_hooks_installed":lifecycle,
        "effective_assets":assets,
        "disabled":disabled,"assets_match":exact_assets,"binary_matches_adapter":binary_matches,
        "reporter":reporter,"reporter_available":reporter_available,"runtime":runtime,"runtime_writable":runtime_writable,
        "installed_runtime":adapter["runtime"],"inventory_directory":states,"inventory_sessions":inventory,
        "installation_ready":ready,"monitoring_prerequisites_ready":ready && inventory.iter().any(|i| i["status"] == "ready"),
        "live_records":records.len(),
        "records":records.iter().map(|r|json!({"instance":r.identity.agent_instance_id,"pid":r.identity.process.pid,"pane":r.identity.pane_key,
            "runner":r.identity.runner_id,"status":r.state.reduced.status})).collect::<Vec<_>>(),
        "magy_state_directory":crate::magy::state_directory().ok(),
        "magy_observer":"Each sidebar reconciles Zellij-backed watches. Use agent magy for observation without a sidebar.",
        "profile_readiness":"Run doctor with --config-dir PROFILE_HOME/.gemini; copied absolute callback commands share the original reporter/runtime.",
        "capabilities":["activity","permissions","background_tasks","aggregate_completion","terminal_failure"],
        "limitations":["No structured question source; no question-pending heuristic.","Idle without Stop does not establish success.",
            "Uncorrelated/concurrent lifecycle outcomes remain conservative.","No transcript-derived title."]}))?
    );
    Ok(())
}
