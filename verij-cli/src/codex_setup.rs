//! Owned Codex callback composition and neutral lifecycle installation.
use crate::agent_setup::{DoctorArgs, SetupArgs};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const CALLBACK: &str = include_str!("../../integrations/codex/callback.py");

pub const SUPPORTED_VERSIONS: &str = crate::codex::SUPPORTED_VERSIONS;
pub const SUPPORTED_EVENTS: &[&str] = crate::codex::SUPPORTED_EVENTS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub commands: BTreeMap<String, String>,
    pub installed_hooks: BTreeMap<String, Value>,
    pub callback_fingerprint: String,
    pub adapter_fingerprint: String,
    pub python_bin: PathBuf,
    #[serde(default)]
    pub uninstalled: Option<bool>,
}

pub fn directory(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    let path = override_dir
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .context("Codex configuration directory unavailable")?;
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

pub fn parse_version(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    let Some(version_part) = trimmed.strip_prefix("codex-cli ") else {
        bail!("invalid Codex version prefix; expected 'codex-cli <version>', got: {trimmed}");
    };
    let version = version_part.trim();
    if !crate::codex::supports_version(version) {
        bail!(
            "unsupported Codex version: {version}; supported: {:?}",
            SUPPORTED_VERSIONS
        );
    }
    Ok(version.to_string())
}

pub fn version(binary: &Path) -> Result<String> {
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
                bail!("Codex version command failed");
            }
            let raw = String::from_utf8(output.stdout)?;
            return parse_version(&raw);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Codex version detection timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub fn is_executable(p: &Path) -> bool {
    p.is_file()
        && p.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn resolve_bin(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        if is_executable(path) {
            return Ok(path.to_path_buf());
        }
        bail!("executable not found or not executable: {}", path.display());
    }
    if path.components().count() > 1 {
        let absolute = std::env::current_dir()?.join(path);
        if is_executable(&absolute) {
            return Ok(absolute);
        }
        bail!(
            "executable not found or not executable: {}",
            absolute.display()
        );
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(path))
        .find(|p| is_executable(p))
        .with_context(|| format!("binary {} unavailable on PATH", path.display()))
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn command_map(python: &Path, callback: &Path, adapter: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for &event in SUPPORTED_EVENTS {
        let cmd = format!(
            "{} {} {} {}",
            quote(&python.to_string_lossy()),
            quote(&callback.to_string_lossy()),
            event,
            quote(&adapter.to_string_lossy())
        );
        map.insert(event.to_string(), cmd);
    }
    map
}

pub fn expected_handler(command: &str) -> Value {
    json!({
        "type": "command",
        "command": command,
        "timeout": 2
    })
}

pub fn expected_group(command: &str) -> Value {
    json!({
        "hooks": [
            expected_handler(command)
        ]
    })
}

fn is_16_hex_chars(s: &str) -> bool {
    s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_hooks_schema(val: &Value, path: &Path) -> Result<()> {
    if !val.is_object() {
        bail!("hooks config root must be an object: {}", path.display());
    }
    if let Some(hooks_field) = val.get("hooks") {
        if !hooks_field.is_object() {
            bail!("'hooks' field must be an object: {}", path.display());
        }
        let hooks_obj = hooks_field.as_object().unwrap();
        for (event_name, group_val) in hooks_obj {
            let groups = group_val.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "event '{}' in hooks must be an array: {}",
                    event_name,
                    path.display()
                )
            })?;
            for (g_idx, group) in groups.iter().enumerate() {
                let g_obj = group.as_object().ok_or_else(|| {
                    anyhow::anyhow!(
                        "group {} in event '{}' must be an object: {}",
                        g_idx,
                        event_name,
                        path.display()
                    )
                })?;
                let hooks_arr = g_obj
                    .get("hooks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "group {} in event '{}' missing 'hooks' array: {}",
                            g_idx,
                            event_name,
                            path.display()
                        )
                    })?;
                for (h_idx, handler) in hooks_arr.iter().enumerate() {
                    let h_obj = handler.as_object().ok_or_else(|| {
                        anyhow::anyhow!(
                            "handler {} in group {} of event '{}' must be an object: {}",
                            h_idx,
                            g_idx,
                            event_name,
                            path.display()
                        )
                    })?;
                    let h_type = h_obj.get("type").and_then(Value::as_str).ok_or_else(|| {
                        anyhow::anyhow!(
                            "handler {} in group {} of event '{}' missing 'type' string: {}",
                            h_idx,
                            g_idx,
                            event_name,
                            path.display()
                        )
                    })?;
                    if h_type == "command" && h_obj.get("command").and_then(Value::as_str).is_none()
                    {
                        bail!(
                            "command handler {} in group {} of event '{}' missing 'command' string: {}",
                            h_idx,
                            g_idx,
                            event_name,
                            path.display()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_hooks_file(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"hooks": {}}));
    }
    let raw = fs::read(path)?;
    let val: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("malformed JSON in {}", path.display()))?;
    validate_hooks_schema(&val, path)?;
    Ok(val)
}

pub fn read_manifest(path: &Path) -> Result<Option<Manifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read(path)?;
    let manifest: Manifest = serde_json::from_slice(&raw)
        .with_context(|| format!("malformed or invalid manifest in {}", path.display()))?;
    if manifest.commands.len() != SUPPORTED_EVENTS.len()
        || manifest.installed_hooks.len() != SUPPORTED_EVENTS.len()
    {
        bail!(
            "manifest in {} has incomplete event definitions",
            path.display()
        );
    }
    for &event in SUPPORTED_EVENTS {
        let cmd = manifest.commands.get(event).ok_or_else(|| {
            anyhow::anyhow!(
                "manifest in {} missing event command {}",
                path.display(),
                event
            )
        })?;
        if cmd.trim().is_empty() {
            bail!(
                "manifest in {} has empty command for event {}",
                path.display(),
                event
            );
        }
        let grp = manifest.installed_hooks.get(event).ok_or_else(|| {
            anyhow::anyhow!(
                "manifest in {} missing installed_hooks for {}",
                path.display(),
                event
            )
        })?;
        if grp != &expected_group(cmd) {
            bail!(
                "manifest in {} has inconsistent installed_hooks for {}",
                path.display(),
                event
            );
        }
    }
    if !is_16_hex_chars(&manifest.callback_fingerprint)
        || !is_16_hex_chars(&manifest.adapter_fingerprint)
    {
        bail!(
            "manifest in {} contains invalid 16-hex fingerprint",
            path.display()
        );
    }
    if !manifest.python_bin.is_absolute() {
        bail!(
            "manifest in {} python_bin must be an absolute path",
            path.display()
        );
    }
    Ok(Some(manifest))
}

pub fn setup(args: SetupArgs) -> Result<()> {
    // 1. Preflight binary and version check
    let codex_bin = resolve_bin(&args.codex_bin)?;
    let version = self::version(&codex_bin)?;
    let python = resolve_bin(&args.python_bin)?;

    // 2. Preflight directory resolution
    let dir = directory(args.config_dir)?;
    fs::create_dir_all(&dir)?;
    let _lock = crate::agent_store::RegistrationLock::acquire(&dir)?;

    let assets = dir.join("verij-monitoring-codex");
    let manifest_path = assets.join("installed.json");
    let callback_path = assets.join("callback.py");
    let adapter_path = assets.join("adapter.json");
    let hooks_path = dir.join("hooks.json");

    // 3. Preflight hooks.json schema validation BEFORE touching any assets
    let mut hook_root = read_hooks_file(&hooks_path)?;
    let initial_hook_root = hook_root.clone();

    // 4. Preflight existing assets and ownership manifest
    let old_manifest = read_manifest(&manifest_path)?;

    if old_manifest.is_none() {
        // Refuse setup if adapter assets exist without ownership manifest
        if callback_path.exists() || adapter_path.exists() {
            bail!(
                "unowned adapter assets exist without ownership manifest in {}; refuse collision",
                assets.display()
            );
        }
    }

    if callback_path.exists() {
        let existing = fs::read_to_string(&callback_path)?;
        let fp = crate::agent_setup::fingerprint(&existing);
        if existing != CALLBACK
            && old_manifest
                .as_ref()
                .map_or(true, |m| m.callback_fingerprint != fp)
        {
            bail!("modified Codex adapter callback asset; preserve edits before setup");
        }
    }

    if adapter_path.exists() {
        let existing = fs::read_to_string(&adapter_path)?;
        let fp = crate::agent_setup::fingerprint(&existing);
        if old_manifest
            .as_ref()
            .map_or(true, |m| m.adapter_fingerprint != fp)
        {
            bail!("modified Codex adapter configuration; preserve edits before setup");
        }
    }

    let commands = command_map(&python, &callback_path, &adapter_path);

    if hook_root.get("hooks").is_none() {
        hook_root["hooks"] = json!({});
    }
    let hooks_obj = hook_root["hooks"].as_object_mut().unwrap();

    let callback_raw = callback_path.to_string_lossy();
    let callback_quoted = quote(&callback_raw);

    // 5. Preflight group semantics and ownership collision in hooks.json
    let mut events_to_install = Vec::new();

    for &event in SUPPORTED_EVENTS {
        let expected_cmd = &commands[event];
        let expected_h = expected_handler(expected_cmd);
        let old_owned_cmd = old_manifest.as_ref().map(|m| &m.commands[event]);

        if let Some(event_groups) = hooks_obj.get_mut(event) {
            let groups = event_groups.as_array_mut().unwrap();
            let mut found_count = 0;
            let mut found_group_idx = None;

            for (g_idx, group) in groups.iter().enumerate() {
                let handlers = group.get("hooks").and_then(Value::as_array).unwrap();
                for h in handlers {
                    let cmd = h.get("command").and_then(Value::as_str);
                    if cmd == Some(expected_cmd.as_str())
                        || (old_owned_cmd.is_some() && cmd == old_owned_cmd.map(String::as_str))
                    {
                        // Match without manifest is an ownership collision
                        if old_manifest.is_none() {
                            bail!(
                                "Verij hook ownership collision without valid manifest for event {}",
                                event
                            );
                        }
                        found_count += 1;
                        found_group_idx = Some(g_idx);
                    } else if old_manifest.is_none() {
                        if let Some(c) = cmd {
                            if c.contains(&*callback_raw) || c.contains(&*callback_quoted) {
                                bail!(
                                    "Verij hook ownership collision without valid manifest for event {}",
                                    event
                                );
                            }
                        }
                    }
                }
            }

            if found_count > 1 {
                bail!("duplicate owned handlers for event {}", event);
            }

            if found_count == 1 {
                let g_idx = found_group_idx.unwrap();
                let group = &mut groups[g_idx];
                let g_obj = group.as_object().unwrap();

                // Group must not have edited matcher or extra configuration
                if g_obj.len() > 1 || g_obj.get("matcher").is_some() {
                    bail!(
                        "edited Verij hook group metadata/matcher for event {}; preserve edits before setup",
                        event
                    );
                }

                // Check handler ENTIRE exact shape against previous expected handler
                let handlers = group
                    .get_mut("hooks")
                    .and_then(Value::as_array_mut)
                    .unwrap();
                let owned_handler_idx = handlers
                    .iter()
                    .position(|h| {
                        let c = h.get("command").and_then(Value::as_str);
                        c == Some(expected_cmd.as_str())
                            || (old_owned_cmd.is_some() && c == old_owned_cmd.map(String::as_str))
                    })
                    .unwrap();

                let h = &mut handlers[owned_handler_idx];
                let cmd_to_match = if let Some(old_cmd) = old_owned_cmd {
                    old_cmd.as_str()
                } else {
                    expected_cmd.as_str()
                };
                let expected_old_h = expected_handler(cmd_to_match);
                if *h != expected_old_h {
                    bail!(
                        "edited Verij hook handler for event {}; preserve edits before setup",
                        event
                    );
                }

                // Update command in memory to expected_h
                *h = expected_h;
            } else {
                events_to_install.push(event);
            }
        } else {
            events_to_install.push(event);
        }
    }

    // 6. Preflight binary device and inode
    let codex_meta = fs::metadata(&codex_bin)?;
    let binary_dev = codex_meta.dev();
    let binary_ino = codex_meta.ino();

    let reporter = std::env::current_exe()?;
    let runtime = crate::agent_store::resolve_agent_runtime_dir()?;
    let states = crate::fs_watcher::resolve_states_dir();

    // 7. Write assets atomically
    fs::create_dir_all(&assets)?;

    let temp_callback = callback_path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    fs::write(&temp_callback, CALLBACK)?;
    fs::rename(temp_callback, &callback_path)?;

    let adapter_val = json!({
        "enabled": true,
        "version": version,
        "reporter": reporter,
        "runtime": runtime,
        "states": states,
        "binary_dev": binary_dev,
        "binary_ino": binary_ino,
    });
    let adapter_str = serde_json::to_string_pretty(&adapter_val)?;
    let adapter_fp = crate::agent_setup::fingerprint(&adapter_str);
    crate::agent_store::atomic_write_json(&adapter_path, &adapter_val)?;

    let mut installed_hooks = BTreeMap::new();
    for &event in SUPPORTED_EVENTS {
        installed_hooks.insert(event.to_string(), expected_group(&commands[event]));
    }
    let manifest = Manifest {
        commands: commands.clone(),
        installed_hooks,
        callback_fingerprint: crate::agent_setup::fingerprint(CALLBACK),
        adapter_fingerprint: adapter_fp,
        python_bin: python,
        uninstalled: Some(false),
    };
    crate::agent_store::atomic_write_json(&manifest_path, &serde_json::to_value(&manifest)?)?;

    // 8. Surgically update hooks.json if any events need addition OR in-memory update occurred
    if !events_to_install.is_empty() {
        for event in events_to_install {
            let cmd = &commands[event];
            let new_grp = expected_group(cmd);
            if let Some(arr) = hooks_obj.get_mut(event).and_then(Value::as_array_mut) {
                arr.push(new_grp);
            } else {
                hooks_obj.insert(event.to_string(), json!([new_grp]));
            }
        }
    }

    if hook_root != initial_hook_root {
        crate::agent_store::atomic_write_json(&hooks_path, &hook_root)?;
    }

    println!(
        "{}",
        json!({
            "installed": true,
            "adapter": "codex",
            "version": version,
            "config_dir": dir,
            "reporter": reporter,
            "runtime": runtime,
            "restart_required": true,
        })
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

    let assets = dir.join("verij-monitoring-codex");
    let manifest_path = assets.join("installed.json");
    let callback_path = assets.join("callback.py");
    let adapter_path = assets.join("adapter.json");
    let hooks_path = dir.join("hooks.json");

    // Fail non-destructively on malformed hooks BEFORE any edits
    let mut hook_root = if hooks_path.exists() {
        Some(read_hooks_file(&hooks_path)?)
    } else {
        None
    };

    // Missing or malformed manifest => preserve all assets and handlers
    let mut manifest = match read_manifest(&manifest_path)? {
        Some(m) => m,
        None => {
            println!("{{\"uninstalled\":false,\"error\":\"missing or unowned manifest; all files preserved\"}}");
            return Ok(());
        }
    };

    let mut has_retained_refs = false;
    let mut hooks_changed = false;
    let callback_raw = callback_path.to_string_lossy();
    let callback_quoted = quote(&callback_raw);
    let adapter_raw = adapter_path.to_string_lossy();
    let adapter_quoted = quote(&adapter_raw);

    if let Some(root) = &mut hook_root {
        if let Some(hooks_obj) = root.get_mut("hooks").and_then(Value::as_object_mut) {
            // First pass: scan ALL events (including extra events) for retained references
            for (event_name, group_val) in hooks_obj.iter() {
                if let Some(groups) = group_val.as_array() {
                    let owned_cmd = manifest.commands.get(event_name);
                    for group in groups {
                        if let Some(handlers) = group.get("hooks").and_then(Value::as_array) {
                            for h in handlers {
                                if let Some(cmd) = h.get("command").and_then(Value::as_str) {
                                    let references_asset = cmd.contains(&*callback_raw)
                                        || cmd.contains(&*callback_quoted)
                                        || cmd.contains(&*adapter_raw)
                                        || cmd.contains(&*adapter_quoted)
                                        || manifest.commands.values().any(|c| c == cmd);

                                    if references_asset {
                                        let is_exact_owned = owned_cmd
                                            .map_or(false, |oc| oc == cmd)
                                            && h == &expected_handler(cmd)
                                            && group.as_object().map_or(false, |o| o.len() == 1);
                                        if !is_exact_owned {
                                            has_retained_refs = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Second pass: remove exact unmodified owned handlers and newly-empty groups
            for &event in SUPPORTED_EVENTS {
                let owned_cmd = manifest.commands.get(event).unwrap();
                let expected_h = expected_handler(owned_cmd);

                if let Some(groups) = hooks_obj.get_mut(event).and_then(Value::as_array_mut) {
                    let mut groups_to_remove = Vec::new();

                    for (g_idx, group) in groups.iter_mut().enumerate() {
                        let g_obj = group.as_object().unwrap();
                        let group_is_unmodified_owned = g_obj.len() == 1;

                        if group_is_unmodified_owned {
                            if let Some(handlers) =
                                group.get_mut("hooks").and_then(Value::as_array_mut)
                            {
                                let mut to_remove = Vec::new();
                                for (h_idx, h) in handlers.iter().enumerate() {
                                    if h.get("command").and_then(Value::as_str)
                                        == Some(owned_cmd.as_str())
                                        && h == &expected_h
                                    {
                                        to_remove.push(h_idx);
                                    }
                                }
                                if !to_remove.is_empty() {
                                    hooks_changed = true;
                                    for idx in to_remove.into_iter().rev() {
                                        handlers.remove(idx);
                                    }
                                    if handlers.is_empty() {
                                        groups_to_remove.push(g_idx);
                                    }
                                }
                            }
                        }
                    }

                    if !groups_to_remove.is_empty() {
                        hooks_changed = true;
                        for idx in groups_to_remove.into_iter().rev() {
                            groups.remove(idx);
                        }
                    }
                }
            }
        }

        if hooks_changed {
            crate::agent_store::atomic_write_json(&hooks_path, root)?;
        }
    }

    // Asset preservation & cleanup
    let callback_edited = match fs::read(&callback_path) {
        Err(_) => false,
        Ok(bytes) => {
            if bytes == CALLBACK.as_bytes() {
                false
            } else {
                match std::str::from_utf8(&bytes) {
                    Ok(text) => {
                        crate::agent_setup::fingerprint(text) != manifest.callback_fingerprint
                    }
                    Err(_) => true,
                }
            }
        }
    };

    let adapter_edited = match fs::read(&adapter_path) {
        Err(_) => false,
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => crate::agent_setup::fingerprint(text) != manifest.adapter_fingerprint,
            Err(_) => true,
        },
    };

    if !callback_edited && !has_retained_refs && callback_path.exists() {
        fs::remove_file(&callback_path)
            .with_context(|| format!("failed to remove {}", callback_path.display()))?;
    }

    if !adapter_edited {
        if has_retained_refs {
            // Keep disabled adapter and update manifest adapter_fingerprint
            if adapter_path.exists() {
                let mut val: Value = serde_json::from_slice(&fs::read(&adapter_path)?)?;
                val["enabled"] = json!(false);
                crate::agent_store::atomic_write_json(&adapter_path, &val)?;
                let updated_str = serde_json::to_string_pretty(&val)?;
                manifest.adapter_fingerprint = crate::agent_setup::fingerprint(&updated_str);
            }
        } else if adapter_path.exists() {
            fs::remove_file(&adapter_path)
                .with_context(|| format!("failed to remove {}", adapter_path.display()))?;
        }
    }

    if !callback_edited && !adapter_edited && !has_retained_refs {
        if manifest_path.exists() {
            fs::remove_file(&manifest_path)
                .with_context(|| format!("failed to remove {}", manifest_path.display()))?;
        }
        if assets.exists() {
            let mut is_empty = true;
            for entry in fs::read_dir(&assets)? {
                let _ = entry?;
                is_empty = false;
                break;
            }
            if is_empty {
                fs::remove_dir(&assets)
                    .with_context(|| format!("failed to remove {}", assets.display()))?;
            }
        }
    } else {
        manifest.uninstalled = Some(true);
        crate::agent_store::atomic_write_json(&manifest_path, &serde_json::to_value(&manifest)?)?;
    }

    println!("{{\"uninstalled\":true,\"restart_required\":true}}");
    Ok(())
}

pub fn doctor(args: DoctorArgs) -> Result<()> {
    let dir = directory(args.config_dir)?;
    let assets = dir.join("verij-monitoring-codex");
    let manifest_path = assets.join("installed.json");
    let callback_path = assets.join("callback.py");
    let adapter_path = assets.join("adapter.json");
    let hooks_path = dir.join("hooks.json");

    let codex_bin = resolve_bin(&args.codex_bin).ok();
    let binary_available = codex_bin.is_some();
    let version = codex_bin.as_ref().and_then(|p| self::version(p).ok());
    let supported_version = version
        .as_deref()
        .is_some_and(crate::codex::supports_version);

    let manifest = read_manifest(&manifest_path).unwrap_or(None);
    let manifest_valid = manifest.is_some();

    let python_available = manifest
        .as_ref()
        .map_or(false, |m| is_executable(&m.python_bin));

    let callback_present = callback_path.exists();
    let callback_intact = callback_present
        && fs::read_to_string(&callback_path).is_ok_and(|s| {
            s == CALLBACK
                && manifest.as_ref().map_or(false, |m| {
                    m.callback_fingerprint == crate::agent_setup::fingerprint(CALLBACK)
                })
        });

    let adapter_raw = fs::read_to_string(&adapter_path).ok();
    let adapter_val = adapter_raw
        .as_ref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());

    let adapter_intact = adapter_val.as_ref().is_some_and(|a| {
        a.get("enabled") == Some(&Value::Bool(true))
            && manifest.as_ref().map_or(false, |m| {
                m.adapter_fingerprint
                    == crate::agent_setup::fingerprint(adapter_raw.as_deref().unwrap_or(""))
            })
    });

    let binary_matches = codex_bin
        .as_ref()
        .and_then(|bin| fs::metadata(bin).ok())
        .zip(adapter_val.as_ref())
        .map_or(false, |(meta, adapter)| {
            adapter["binary_dev"].as_u64() == Some(meta.dev())
                && adapter["binary_ino"].as_u64() == Some(meta.ino())
        });

    let adapter_version_compatible = adapter_val
        .as_ref()
        .and_then(|a| a.get("version").and_then(Value::as_str))
        .is_some_and(crate::codex::supports_version);

    let reporter_from_adapter = adapter_val
        .as_ref()
        .and_then(|a| a.get("reporter").and_then(Value::as_str));
    let reporter_version =
        reporter_from_adapter.and_then(|p| crate::agent_setup::version(Path::new(p)).ok());
    let reporter_available = reporter_version
        .as_deref()
        .is_some_and(|v| v.starts_with("verij "));

    let hook_root = read_hooks_file(&hooks_path).ok();
    let hooks_installed = hook_root
        .as_ref()
        .zip(manifest.as_ref())
        .map_or(false, |(root, man)| {
            if let Some(hooks_obj) = root.get("hooks").and_then(Value::as_object) {
                SUPPORTED_EVENTS.iter().all(|&event| {
                    let owned_cmd = man.commands.get(event);
                    hooks_obj
                        .get(event)
                        .and_then(Value::as_array)
                        .map_or(false, |groups| {
                            let mut owned_count = 0;
                            for g in groups {
                                let g_obj = g.as_object().unwrap();
                                if let Some(handlers) = g_obj.get("hooks").and_then(Value::as_array)
                                {
                                    for h in handlers {
                                        if owned_cmd.is_some_and(|oc| {
                                            h.get("command").and_then(Value::as_str) == Some(oc)
                                        }) {
                                            if g_obj.len() == 1
                                                && h == &expected_handler(owned_cmd.unwrap())
                                            {
                                                owned_count += 1;
                                            } else {
                                                return false;
                                            }
                                        }
                                    }
                                }
                            }
                            owned_count == 1
                        })
                })
            } else {
                false
            }
        });

    let effective_runtime = std::env::var_os("VERIJ_AGENT_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            adapter_val
                .as_ref()
                .and_then(|a| a.get("runtime").and_then(Value::as_str).map(PathBuf::from))
        })
        .or_else(|| crate::agent_store::resolve_agent_runtime_dir().ok());

    let runtime_writable = effective_runtime.as_ref().map_or(false, |rt| {
        if !rt.is_absolute() {
            return false;
        }
        if fs::create_dir_all(rt).is_err() {
            return false;
        }
        let check = rt.join(format!(".doctor-{}", uuid::Uuid::new_v4()));
        match crate::agent_store::atomic_write_json(&check, &json!({"self_check": true})) {
            Ok(()) => {
                let _ = fs::remove_file(&check);
                true
            }
            Err(_) => false,
        }
    });

    let states = crate::fs_watcher::resolve_states_dir();
    let effective_states = std::env::var_os("VERIJ_STATES_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            adapter_val
                .as_ref()
                .and_then(|a| a.get("states").and_then(Value::as_str).map(PathBuf::from))
        })
        .unwrap_or_else(|| states.clone());

    let mut snapshots = if effective_states.is_dir() {
        crate::inventory::read_states(&effective_states)
    } else {
        Vec::new()
    };
    if let Some(session) = args.session.as_ref() {
        snapshots.retain(|s| &s.name == session);
    }
    let inventory_sessions = crate::agent_setup::inventory_diagnostics(&snapshots);
    let inventory_ready = inventory_sessions.iter().any(|s| s["status"] == "ready");

    let mut installation_error = None;
    if !binary_available {
        installation_error = Some("Codex binary unavailable or not executable".to_string());
    } else if !supported_version {
        installation_error = Some("Codex version unsupported or missing".to_string());
    } else if !manifest_valid {
        installation_error =
            Some("ownership manifest installed.json missing or invalid".to_string());
    } else if !python_available {
        installation_error =
            Some("configured Python binary unavailable or not executable".to_string());
    } else if !callback_intact {
        installation_error = Some("callback.py missing or modified".to_string());
    } else if !adapter_intact {
        installation_error = Some("adapter.json missing, disabled, or modified".to_string());
    } else if !adapter_version_compatible {
        installation_error =
            Some("Codex installed adapter contract version is unsupported".to_string());
    } else if !hooks_installed {
        installation_error =
            Some("Codex lifecycle hooks missing, unowned, or modified in hooks.json".to_string());
    } else if !reporter_available {
        installation_error = Some("Verij reporter binary unavailable or incompatible".to_string());
    }

    let installation_ready = installation_error.is_none();
    let monitoring_prerequisites_ready = installation_ready && inventory_ready && runtime_writable;

    let mut records = effective_runtime
        .as_ref()
        .map(|rt| crate::agent_watcher::read_records(&rt.join("agents/v1")))
        .unwrap_or_default();

    records.retain(|r| {
        r.identity.kind == verij_types::agent::AgentKind::Codex
            && !r.identity.is_synthetic
            && crate::process::is_alive(&r.identity.process)
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
        serde_json::to_string_pretty(&json!({
            "adapter": "codex",
            "version": version,
            "supported_version": supported_version,
            "supported_versions": [SUPPORTED_VERSIONS],
            "binary_available": binary_available,
            "binary_matches": binary_matches,
            "python_available": python_available,
            "config_dir": dir,
            "callback_present": callback_present,
            "callback_intact": callback_intact,
            "adapter_intact": adapter_intact,
            "hooks_installed": hooks_installed,
            "reporter": reporter_from_adapter,
            "reporter_available": reporter_available,
            "reporter_version": reporter_version,
            "runtime": effective_runtime,
            "runtime_writable": runtime_writable,
            "installed_runtime": adapter_val.as_ref().and_then(|a| a.get("runtime")),
            "inventory_directory": states,
            "effective_inventory_directory": effective_states,
            "inventory_sessions": inventory_sessions,
            "inventory_ready": inventory_ready,
            "requested_session": args.session,
            "installation_ready": installation_ready,
            "installation_error": installation_error,
            "monitoring_prerequisites_ready": monitoring_prerequisites_ready,
            "active_instances": records.len(),
            "interactive_mode": "--no-daemon",
            "hook_trust_required": true,
            "hook_trust_status": "unknown (manual /hooks needed)",
            "native_terminal_errors_supported": false,
            "limitations": [
                "Native provider errors are unsupported; provider failures do not trigger Stop hooks.",
                "Codex hook Stop is a stop boundary potentially followed by third-party continuation.",
                "TUI requires interactive mode with --no-daemon; native exec invokes hooks, but adapter intentionally skips headless/daemon execution to avoid foreign pane interference.",
                "Codex hook trust requires manual approval via /hooks in interactive session."
            ],
            "note": "Installation, runtime, and pane-inventory prerequisite checks. active_instances confirms actual live reporting. Native terminal errors unsupported; hook Stop is a stop boundary."
        }))?
    );
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    pub fn test_version_parser() {
        assert_eq!(parse_version("codex-cli 0.162.1\n").unwrap(), "0.162.1");
        assert_eq!(parse_version("codex-cli 0.160.1").unwrap(), "0.160.1");
        assert_eq!(parse_version("codex-cli 0.200.2").unwrap(), "0.200.2");
        assert!(parse_version("codex-cli 1.0.0").is_err());
        assert!(parse_version("0.162.1").is_err());
        assert!(parse_version("codex 0.162.1").is_err());
        assert!(parse_version("codex-cli 0.99.0").is_err());
        assert!(parse_version("openai-codex 0.162.1").is_err());
    }
}
