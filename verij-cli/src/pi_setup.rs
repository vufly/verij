/// verij-cli/src/pi_setup.rs
///
/// Pi 1.2.0 agent monitoring lifecycle adapter and diagnostic doctor.
/// Dispatches setup, uninstall, and doctor for Pi.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

const INDEX: &str = include_str!("../../integrations/pi/index.ts");
const CORE: &str = include_str!("../../integrations/pi/core.mjs");
pub const SUPPORTED_VERSION: &str = crate::pi::SUPPORTED_VERSION;
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const ADAPTER_ID: &str = "verij-monitoring-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PiManifest {
    pub schema_version: u32,
    pub adapter_id: String,
    pub index_fingerprint: String,
    pub core_fingerprint: String,
    pub adapter_fingerprint: String,
    pub reporter: PathBuf,
    pub version: String,
    #[serde(default)]
    pub installed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstalled: Option<bool>,
}

fn is_valid_hex_16(s: &str) -> bool {
    s.len() == 16
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Resolve Pi agent directory following precedence:
/// explicit override (`--config-dir`) > `PI_CODING_AGENT_DIR` > `~/.pi/agent`.
/// Expands `~` and `~/...` to `$HOME`, canonicalizes nonexistent targets through existing prefix.
/// Explicitly rejects `file:` URLs before writes.
pub fn resolve_agent_dir(override_dir: Option<PathBuf>, ensure_created: bool) -> Result<PathBuf> {
    let raw = if let Some(dir) = override_dir {
        dir
    } else if let Some(env_dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        let s = env_dir.to_string_lossy();
        if s.is_empty() {
            let home = std::env::var_os("HOME").context("HOME environment variable unavailable")?;
            PathBuf::from(home).join(".pi/agent")
        } else {
            PathBuf::from(env_dir)
        }
    } else {
        let home = std::env::var_os("HOME").context("HOME environment variable unavailable")?;
        PathBuf::from(home).join(".pi/agent")
    };

    let raw_str = raw.to_string_lossy();
    if raw_str.starts_with("file:") {
        bail!(
            "file: URLs are not supported for agent configuration directory ({}); please specify an explicit local filesystem path (e.g. /path/to/agent or ~/.pi/agent)",
            raw_str
        );
    }

    let expanded = expand_tilde(&raw)?;
    let canonical = canonicalize_path(&expanded)?;

    if ensure_created {
        fs::create_dir_all(&canonical)?;
    }

    Ok(canonical)
}

pub(crate) fn expand_tilde(path: &Path) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();
    if path_str == "~" {
        let home = std::env::var_os("HOME").context("HOME environment variable unavailable")?;
        return Ok(PathBuf::from(home));
    }
    if let Some(rest) = path_str.strip_prefix("~/") {
        let home = std::env::var_os("HOME").context("HOME environment variable unavailable")?;
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(path.to_path_buf())
}

pub fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut current = PathBuf::new();
    let mut pending_nonexistent: Vec<PathBuf> = Vec::new();
    let mut in_nonexistent = false;

    for component in abs.components() {
        match component {
            Component::Prefix(p) => {
                current.push(p.as_os_str());
            }
            Component::RootDir => {
                current.push(std::path::MAIN_SEPARATOR.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if in_nonexistent {
                    if let Some(last) = pending_nonexistent.last() {
                        if last == Path::new("..") {
                            pending_nonexistent.push(PathBuf::from(".."));
                        } else {
                            pending_nonexistent.pop();
                            if pending_nonexistent.is_empty() {
                                in_nonexistent = false;
                            }
                        }
                    } else if current.parent().is_some() {
                        current.pop();
                        if let Ok(c) = current.canonicalize() {
                            current = c;
                        }
                    }
                } else {
                    current.pop();
                    if let Ok(c) = current.canonicalize() {
                        current = c;
                    }
                }
            }
            Component::Normal(c) => {
                if in_nonexistent {
                    pending_nonexistent.push(PathBuf::from(c));
                } else {
                    let next = current.join(c);
                    if let Ok(c) = next.canonicalize() {
                        current = c;
                    } else {
                        in_nonexistent = true;
                        pending_nonexistent.push(PathBuf::from(c));
                    }
                }
            }
        }
    }

    let mut result = current;
    for c in pending_nonexistent {
        result.push(c);
    }
    Ok(result)
}

pub(crate) fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Drain and cap version output without blocking wait_with_output on inherited pipe holders.
/// Drains bounded nonblocking buffered output completely before checking exit, enforces total cap and deadline.
pub fn probe_version(binary: &Path) -> Result<String> {
    use std::io::Read;
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(binary);
    cmd.arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn binary: {}", binary.display()))?;

    let child_pid = child.id() as i32;
    let mut stdout = child.stdout.take().context("missing stdout pipe")?;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(3);

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = stdout.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
            if flags < 0 {
                let _ = child.kill();
                let _ = child.wait();
                bail!("failed to get pipe flags via fcntl: {}", binary.display());
            }
            if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "failed to set nonblocking on pipe via fcntl: {}",
                    binary.display()
                );
            }
        }
    }

    let kill_group = |pid: i32, child: &mut std::process::Child| {
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    };

    let mut output_bytes = Vec::new();
    let mut buf = [0u8; 512];
    let mut stdout_closed = false;

    loop {
        if start.elapsed() >= timeout {
            kill_group(child_pid, &mut child);
            bail!("version probe timed out: {}", binary.display());
        }

        // Drain bounded nonblocking buffered output to EOF or WouldBlock
        while !stdout_closed {
            match stdout.read(&mut buf) {
                Ok(0) => {
                    stdout_closed = true;
                    break;
                }
                Ok(n) => {
                    output_bytes.extend_from_slice(&buf[..n]);
                    if output_bytes.len() > 4096 {
                        kill_group(child_pid, &mut child);
                        bail!(
                            "version output exceeded maximum allowed size: {}",
                            binary.display()
                        );
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    kill_group(child_pid, &mut child);
                    return Err(e).context("failed reading binary version stdout");
                }
            }
        }

        if let Some(status) = child.try_wait()? {
            // Child exited: drain any remaining buffered output in pipe before finishing
            while !stdout_closed {
                match stdout.read(&mut buf) {
                    Ok(0) => {
                        break;
                    }
                    Ok(n) => {
                        output_bytes.extend_from_slice(&buf[..n]);
                        if output_bytes.len() > 4096 {
                            kill_group(child_pid, &mut child);
                            bail!(
                                "version output exceeded maximum allowed size: {}",
                                binary.display()
                            );
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }

            kill_group(child_pid, &mut child);

            if !status.success() {
                bail!("version probe failed with status: {status}");
            }
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let full_output =
        String::from_utf8(output_bytes).context("version output contains invalid UTF-8")?;
    let version_str = full_output
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(version_str)
}

pub(crate) fn version(binary: &Path) -> Result<String> {
    probe_version(binary)
}

fn read_bounded_bytes(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    if !path.exists() {
        return Ok(None);
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        bail!("symlink detected at {}; refusing to read", path.display());
    }
    if !meta.file_type().is_file() {
        bail!(
            "special file (FIFO/socket/device) detected at {}; refusing to read",
            path.display()
        );
    }
    if meta.len() > max_bytes as u64 {
        bail!(
            "file {} exceeds size limit of {} bytes",
            path.display(),
            max_bytes
        );
    }

    let file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take((max_bytes + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        bail!(
            "file {} exceeds size limit of {} bytes",
            path.display(),
            max_bytes
        );
    }
    Ok(Some(bytes))
}

fn check_no_symlinks_or_special_files(dir: &Path) -> Result<()> {
    if !dir.exists() {
        if let Ok(meta) = fs::symlink_metadata(dir) {
            if meta.file_type().is_symlink() {
                bail!("symlink detected at {}; refusing to operate", dir.display());
            }
        }
        return Ok(());
    }
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        bail!("symlink detected at {}; refusing to operate", dir.display());
    }
    if !meta.is_dir() {
        bail!(
            "expected directory at {}; found non-directory",
            dir.display()
        );
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            bail!(
                "symlink detected at {}; refusing to operate",
                path.display()
            );
        }
        if meta.is_dir() {
            check_no_symlinks_or_special_files(&path)?;
        } else if !meta.file_type().is_file() {
            bail!(
                "special file (FIFO/socket/device) detected at {}; refusing to operate",
                path.display()
            );
        }
    }
    Ok(())
}

fn has_extra_files(dir: &Path, allowed_names: &[&str]) -> Result<bool> {
    if !dir.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !allowed_names.contains(&name_str.as_ref()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_config_path(path: &Path, name: &str) -> Result<()> {
    if !path.is_absolute() {
        bail!("{name} must be an absolute path: {}", path.display());
    }
    let s = path
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8: {}", path.display()))?;
    if s.is_empty() || s.len() > 4096 {
        bail!("{name} length out of bounds (1..4096): {}", s.len());
    }
    if s.starts_with('~') {
        bail!("{name} must not start with '~': {}", s);
    }
    for b in s.bytes() {
        if b < 0x20 || b == 0x7F {
            bail!("{name} contains control character: 0x{:02x}", b);
        }
    }
    Ok(())
}

fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("no parent directory")?;
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            bail!("refusing to write through symlink: {}", path.display());
        }
    }
    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("file");
    let temp_name = format!(".{}.tmp.{}", file_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);
    let write_res = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if write_res.is_err() && temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }
    write_res
}

pub fn read_manifest(manifest_path: &Path) -> Result<Option<PiManifest>> {
    let bytes = match read_bounded_bytes(manifest_path, 32 * 1024)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let manifest: PiManifest = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Ok(None);
    }
    if manifest.adapter_id != ADAPTER_ID {
        return Ok(None);
    }
    if manifest.version != SUPPORTED_VERSION {
        return Ok(None);
    }
    if !is_valid_hex_16(&manifest.index_fingerprint)
        || !is_valid_hex_16(&manifest.core_fingerprint)
        || !is_valid_hex_16(&manifest.adapter_fingerprint)
    {
        return Ok(None);
    }
    if validate_config_path(&manifest.reporter, "reporter").is_err() {
        return Ok(None);
    }
    Ok(Some(manifest))
}

pub fn setup(args: crate::agent_setup::SetupArgs) -> Result<()> {
    let ver = version(&args.pi_bin)?;
    if !crate::pi::supports_version(&ver) {
        bail!(
            "unsupported Pi version: {ver}; supported: [\"{}\"]",
            SUPPORTED_VERSION
        );
    }

    let agent_dir = resolve_agent_dir(args.config_dir, true)?;
    let _lock = crate::agent_store::RegistrationLock::acquire(&agent_dir)?;

    let extensions_parent = agent_dir.join("extensions");
    if let Ok(meta) = fs::symlink_metadata(&extensions_parent) {
        if meta.file_type().is_symlink() {
            bail!(
                "parent extensions directory {} is a symlink; refusing to operate",
                extensions_parent.display()
            );
        }
    }

    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");
    let index_path = ext_dir.join("index.ts");
    let core_path = ext_dir.join("core.mjs");
    let adapter_path = cfg_dir.join("adapter.json");
    let manifest_path = cfg_dir.join("installed.json");

    check_no_symlinks_or_special_files(&ext_dir)?;
    check_no_symlinks_or_special_files(&cfg_dir)?;

    let existing_manifest = read_manifest(&manifest_path)?;

    // Adoption Preflight:
    // If either ext_dir or cfg_dir exists, setup requires a valid typed ownership manifest
    // with matching schema and adapter_id. Unowned directories (missing manifest, empty {},
    // null, or unrelated extra files) must never be adopted or changed.
    if ext_dir.exists() || cfg_dir.exists() {
        if existing_manifest.is_none() {
            bail!(
                "unowned extension directory or configuration found at {}; missing valid ownership manifest; refusing to adopt",
                ext_dir.display()
            );
        }
        if has_extra_files(&ext_dir, &["index.ts", "core.mjs"])? {
            bail!(
                "extension directory contains unrecognized extra files: {}; refusing to adopt",
                ext_dir.display()
            );
        }
        if has_extra_files(&cfg_dir, &["adapter.json", "installed.json"])? {
            bail!(
                "configuration directory contains unrecognized extra files: {}; refusing to adopt",
                cfg_dir.display()
            );
        }
    }

    let current_index = read_bounded_bytes(&index_path, 64 * 1024)?;
    let current_core = read_bounded_bytes(&core_path, 64 * 1024)?;
    let current_adapter = read_bounded_bytes(&adapter_path, 16 * 1024)?;

    if let Some(ref m) = existing_manifest {
        if let Some(ref b) = current_index {
            if b.as_slice() != INDEX.as_bytes() && fingerprint_bytes(b) != m.index_fingerprint {
                bail!(
                    "modified adapter asset {}; preserve edits before setup",
                    index_path.display()
                );
            }
        }
        if let Some(ref b) = current_core {
            if b.as_slice() != CORE.as_bytes() && fingerprint_bytes(b) != m.core_fingerprint {
                bail!(
                    "modified adapter asset {}; preserve edits before setup",
                    core_path.display()
                );
            }
        }
        if let Some(ref b) = current_adapter {
            if fingerprint_bytes(b) != m.adapter_fingerprint {
                bail!(
                    "modified adapter configuration {}; preserve edits before setup",
                    adapter_path.display()
                );
            }
        }
    }

    // Resolve defaults and validate canonical absolute paths
    let reporter_raw = std::env::var_os("VERIJ_BIN")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("verij"));
    let reporter = canonicalize_path(&reporter_raw)?;
    validate_config_path(&reporter, "reporter")?;

    let runtime_dir = canonicalize_path(&crate::agent_store::resolve_agent_runtime_dir()?)?;
    validate_config_path(&runtime_dir, "runtime_dir")?;

    let states_dir = canonicalize_path(&crate::fs_watcher::resolve_states_dir())?;
    validate_config_path(&states_dir, "states_dir")?;

    fs::create_dir_all(&ext_dir)?;
    fs::create_dir_all(&cfg_dir)?;

    let mut changed = false;

    // Write index.ts
    if current_index.as_deref() != Some(INDEX.as_bytes()) {
        atomic_write_text(&index_path, INDEX)?;
        changed = true;
    }

    // Write core.mjs
    if current_core.as_deref() != Some(CORE.as_bytes()) {
        atomic_write_text(&core_path, CORE)?;
        changed = true;
    }

    // Build adapter.json
    let adapter_doc = json!({
        "reporter": reporter,
        "runtime_dir": runtime_dir,
        "states_dir": states_dir
    });
    let adapter_content = format!("{:#}\n", adapter_doc);
    let adapter_bytes = adapter_content.as_bytes();

    if current_adapter.as_deref() != Some(adapter_bytes) {
        atomic_write_text(&adapter_path, &adapter_content)?;
        changed = true;
    }

    // Build or preserve installed.json manifest
    let index_fp = fingerprint_bytes(INDEX.as_bytes());
    let core_fp = fingerprint_bytes(CORE.as_bytes());
    let adapter_fp = fingerprint_bytes(adapter_bytes);

    let installed_at_ms = existing_manifest
        .as_ref()
        .map(|m| m.installed_at_ms)
        .unwrap_or_else(crate::agent_store::now_ms);

    let manifest = PiManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        adapter_id: ADAPTER_ID.to_string(),
        index_fingerprint: index_fp,
        core_fingerprint: core_fp,
        adapter_fingerprint: adapter_fp,
        reporter: reporter.clone(),
        version: SUPPORTED_VERSION.to_string(),
        installed_at_ms,
        uninstalled: None,
    };

    let manifest_content = format!("{:#}\n", json!(manifest));
    let manifest_bytes = manifest_content.as_bytes();

    let current_manifest_bytes = read_bounded_bytes(&manifest_path, 32 * 1024)?;
    if current_manifest_bytes.as_deref() != Some(manifest_bytes) {
        atomic_write_text(&manifest_path, &manifest_content)?;
        changed = true;
    }

    println!(
        "{}",
        json!({
            "installed": true,
            "adapter": "pi",
            "version": ver,
            "config_dir": agent_dir,
            "extensions_dir": ext_dir,
            "changed": changed,
            "restart_required": changed,
            "note": "Restart Pi to load the new or updated extension."
        })
    );
    Ok(())
}

pub fn uninstall(args: crate::agent_setup::SetupArgs) -> Result<()> {
    let agent_dir = resolve_agent_dir(args.config_dir, false)?;
    if !agent_dir.exists() {
        println!("{{\"uninstalled\":true}}");
        return Ok(());
    }
    let _lock = crate::agent_store::RegistrationLock::acquire(&agent_dir)?;

    let extensions_parent = agent_dir.join("extensions");
    if let Ok(meta) = fs::symlink_metadata(&extensions_parent) {
        if meta.file_type().is_symlink() {
            bail!(
                "parent extensions directory {} is a symlink; refusing to operate",
                extensions_parent.display()
            );
        }
    }

    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");
    let index_path = ext_dir.join("index.ts");
    let core_path = ext_dir.join("core.mjs");
    let adapter_path = cfg_dir.join("adapter.json");
    let manifest_path = cfg_dir.join("installed.json");

    if !ext_dir.exists() && !cfg_dir.exists() {
        println!("{{\"uninstalled\":true}}");
        return Ok(());
    }

    check_no_symlinks_or_special_files(&ext_dir)?;
    check_no_symlinks_or_special_files(&cfg_dir)?;

    let manifest = read_manifest(&manifest_path)?;
    let Some(mut manifest) = manifest else {
        bail!("refusing to uninstall unowned Pi adapter: missing or invalid ownership manifest; preserved all files");
    };

    let index_bytes = read_bounded_bytes(&index_path, 64 * 1024)?;
    let core_bytes = read_bounded_bytes(&core_path, 64 * 1024)?;
    let adapter_bytes = read_bounded_bytes(&adapter_path, 16 * 1024)?;

    let index_modified = index_bytes.as_ref().is_some_and(|b| {
        b != INDEX.as_bytes() && fingerprint_bytes(b) != manifest.index_fingerprint
    });
    let core_modified = core_bytes
        .as_ref()
        .is_some_and(|b| b != CORE.as_bytes() && fingerprint_bytes(b) != manifest.core_fingerprint);
    let adapter_modified = adapter_bytes
        .as_ref()
        .is_some_and(|b| fingerprint_bytes(b) != manifest.adapter_fingerprint);

    let has_extra_ext = has_extra_files(&ext_dir, &["index.ts", "core.mjs"])?;
    let has_extra_cfg = has_extra_files(&cfg_dir, &["adapter.json", "installed.json"])?;

    let mut preserved = Vec::new();

    // Conservatively retain entire code/config group if any code or extra user file remains in either directory
    if index_modified || core_modified || has_extra_ext || has_extra_cfg {
        if ext_dir.exists() {
            if let Ok(entries) = fs::read_dir(&ext_dir) {
                for entry in entries.flatten() {
                    preserved.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        if cfg_dir.exists() {
            if let Ok(entries) = fs::read_dir(&cfg_dir) {
                for entry in entries.flatten() {
                    preserved.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        preserved.sort();
        preserved.dedup();
        println!(
            "{}",
            json!({
                "uninstalled": false,
                "restart_required": false,
                "preserved": preserved,
                "note": "Extension code, configuration, or extra files retained; adapter remains active"
            })
        );
        return Ok(());
    }

    // Both code assets are unmodified and no extra files in either directory: safe to remove code
    if index_path.exists() {
        fs::remove_file(&index_path)?;
    }
    if core_path.exists() {
        fs::remove_file(&core_path)?;
    }
    if ext_dir.exists() {
        let _ = fs::remove_dir(&ext_dir);
    }
    let _ = fs::remove_dir(agent_dir.join("extensions"));

    // Code removed; now inspect adapter.json
    if adapter_modified {
        if adapter_path.exists() {
            preserved.push("adapter.json".to_string());
        }
        manifest.uninstalled = Some(true);
        crate::agent_store::atomic_write_json(&manifest_path, &manifest)?;
        println!(
            "{}",
            json!({
                "uninstalled": true,
                "restart_required": true,
                "preserved": preserved
            })
        );
        return Ok(());
    }

    if adapter_path.exists() {
        fs::remove_file(&adapter_path)?;
    }
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    if cfg_dir.exists() {
        let _ = fs::remove_dir(&cfg_dir);
    }

    println!("{{\"uninstalled\":true,\"restart_required\":true}}");
    Ok(())
}

pub(crate) fn segment_glob(pat: &str, target: &str) -> bool {
    let p_segs: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    let t_segs: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();

    fn match_segs(p: &[&str], t: &[&str]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        if p[0] == "**" {
            for i in 0..=t.len() {
                if match_segs(&p[1..], &t[i..]) {
                    return true;
                }
            }
            return false;
        }
        if t.is_empty() {
            return false;
        }
        if single_segment_match(p[0], t[0]) {
            return match_segs(&p[1..], &t[1..]);
        }
        false
    }

    fn single_segment_match(pattern: &str, target: &str) -> bool {
        let p = pattern.as_bytes();
        let t = target.as_bytes();
        let mut p_idx = 0;
        let mut t_idx = 0;
        let mut star_p = None;
        let mut star_t = 0;

        while t_idx < t.len() {
            if p_idx < p.len() && (p[p_idx] == t[t_idx] || p[p_idx] == b'?') {
                p_idx += 1;
                t_idx += 1;
            } else if p_idx < p.len() && p[p_idx] == b'*' {
                star_p = Some(p_idx);
                p_idx += 1;
                star_t = t_idx;
            } else if let Some(sp) = star_p {
                p_idx = sp + 1;
                star_t += 1;
                t_idx = star_t;
            } else {
                return false;
            }
        }
        while p_idx < p.len() && p[p_idx] == b'*' {
            p_idx += 1;
        }
        p_idx == p.len()
    }

    match_segs(&p_segs, &t_segs)
}

/// Evaluates extension enablement according to Pi's exact `isEnabledByOverrides` contract:
/// 1. All `!` glob excludes evaluated first against rel, basename, and posix path.
/// 2. All `+` exact includes evaluated second (re-enables if matched).
/// 3. All `-` exact excludes evaluated last (force-disables, runs AFTER +, order-independent).
/// Minus patterns are literal exact strings, NOT globs.
pub(crate) fn is_enabled_by_overrides(
    ext_dir_abs: &Path,
    agent_dir: &Path,
    entries: &[Value],
) -> (bool, Option<String>, Vec<String>) {
    let mut excludes = Vec::new();
    let mut force_includes = Vec::new();
    let mut force_excludes = Vec::new();
    let mut uncertainties = Vec::new();

    for entry in entries {
        if let Some(s) = entry.as_str() {
            if let Some(excl) = s.strip_prefix('!') {
                excludes.push((excl.to_string(), s.to_string()));
            } else if let Some(inc) = s.strip_prefix('+') {
                force_includes.push((inc.to_string(), s.to_string()));
            } else if let Some(excl) = s.strip_prefix('-') {
                force_excludes.push((excl.to_string(), s.to_string()));
            }
        }
    }

    let rel = ext_dir_abs
        .strip_prefix(agent_dir)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "extensions/verij-monitoring".to_string());
    let name = ext_dir_abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "verij-monitoring".to_string());
    let file_path_posix = ext_dir_abs.to_string_lossy().to_string();

    let normalize_exact = |pat: &str| -> String {
        let expanded =
            if let Some(rest) = pat.strip_prefix("./").or_else(|| pat.strip_prefix(".\\")) {
                rest.to_string()
            } else if let Ok(p) = expand_tilde(Path::new(pat)) {
                p.to_string_lossy().to_string()
            } else {
                pat.to_string()
            };
        expanded.trim_end_matches('/').to_string()
    };

    let mut enabled = true;
    let mut matching_reason = None;

    // 1. Evaluate ! glob excludes
    for (pat, orig) in &excludes {
        if pat.contains("@(")
            || pat.contains("+(")
            || pat.contains('{')
            || pat.contains("!(")
            || pat.contains('[')
        {
            uncertainties.push(format!(
                "Complex glob exclusion pattern '{orig}' cannot be evaluated deterministically; loading status uncertain"
            ));
            continue;
        }
        let norm_pat = if pat.starts_with("./") {
            &pat[2..]
        } else {
            pat
        };
        let norm_pat = norm_pat.trim_end_matches('/');
        if segment_glob(norm_pat, &rel)
            || segment_glob(norm_pat, &name)
            || segment_glob(norm_pat, &file_path_posix)
        {
            enabled = false;
            matching_reason = Some(orig.clone());
        }
    }

    // 2. Evaluate + exact includes
    for (pat, _orig) in &force_includes {
        let norm = normalize_exact(pat);
        if norm == rel || norm == file_path_posix {
            enabled = true;
            matching_reason = None;
        }
    }

    // 3. Evaluate - exact excludes
    for (pat, orig) in &force_excludes {
        let norm = normalize_exact(pat);
        if norm == rel || norm == file_path_posix {
            enabled = false;
            matching_reason = Some(orig.clone());
        }
    }

    (enabled, matching_reason, uncertainties)
}

pub fn doctor(args: crate::agent_setup::DoctorArgs) -> Result<()> {
    let agent_dir = resolve_agent_dir(args.config_dir, false)?;
    let ext_dir = agent_dir.join("extensions/verij-monitoring");
    let cfg_dir = agent_dir.join("verij-monitoring");
    let index_path = ext_dir.join("index.ts");
    let core_path = ext_dir.join("core.mjs");
    let adapter_path = cfg_dir.join("adapter.json");
    let manifest_path = cfg_dir.join("installed.json");

    let ver_res = version(&args.pi_bin);
    let ver = ver_res.as_ref().ok().cloned();
    let binary_available = ver.is_some();
    let version_supported = ver.as_deref().is_some_and(crate::pi::supports_version);

    let mut symlinks = Vec::new();
    let extensions_parent = agent_dir.join("extensions");
    if let Ok(meta) = fs::symlink_metadata(&extensions_parent) {
        if meta.file_type().is_symlink() {
            symlinks.push("extensions_parent".to_string());
        }
    }
    for (name, path) in [
        ("extensions_dir", &ext_dir),
        ("config_dir", &cfg_dir),
        ("index.ts", &index_path),
        ("core.mjs", &core_path),
        ("adapter.json", &adapter_path),
        ("installed.json", &manifest_path),
    ] {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                symlinks.push(name.to_string());
            }
        }
    }
    let symlink_detected = !symlinks.is_empty();

    let (manifest, manifest_err) = if !symlinks.contains(&"installed.json".to_string()) {
        match read_manifest(&manifest_path) {
            Ok(m) => (m, None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, Some("manifest is a symlink".to_string()))
    };
    let manifest_valid = manifest.is_some() && manifest_err.is_none();

    let index_bytes = read_bounded_bytes(&index_path, 64 * 1024).unwrap_or(None);
    let index_intact = index_bytes.as_deref() == Some(INDEX.as_bytes())
        && manifest.as_ref().is_some_and(|m| {
            m.index_fingerprint == fingerprint_bytes(INDEX.as_bytes())
                && m.uninstalled != Some(true)
        });

    let core_bytes = read_bounded_bytes(&core_path, 64 * 1024).unwrap_or(None);
    let core_intact = core_bytes.as_deref() == Some(CORE.as_bytes())
        && manifest.as_ref().is_some_and(|m| {
            m.core_fingerprint == fingerprint_bytes(CORE.as_bytes()) && m.uninstalled != Some(true)
        });

    let exact_assets = index_intact && core_intact;

    let adapter_bytes = read_bounded_bytes(&adapter_path, 16 * 1024).unwrap_or(None);
    let adapter_val: Option<Value> = adapter_bytes
        .as_ref()
        .and_then(|b| serde_json::from_slice(b).ok());

    let adapter_intact = adapter_val.is_some()
        && manifest.as_ref().is_some_and(|m| {
            adapter_bytes
                .as_ref()
                .is_some_and(|b| fingerprint_bytes(b) == m.adapter_fingerprint)
                && m.uninstalled != Some(true)
        });

    // Reporter resolution: launch env VERIJ_BIN > adapter.json > "verij"
    let reporter_override = std::env::var_os("VERIJ_BIN").map(PathBuf::from);
    let effective_reporter = reporter_override
        .or_else(|| {
            adapter_val
                .as_ref()
                .and_then(|a| a.get("reporter"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("verij"));

    let reporter_version = probe_version(&effective_reporter).ok();
    let reporter_available = reporter_version
        .as_deref()
        .is_some_and(|v| v.starts_with("verij "));

    let current_runtime =
        canonicalize_path(&crate::agent_store::resolve_agent_runtime_dir()?).unwrap_or_default();
    let effective_runtime = adapter_val
        .as_ref()
        .and_then(|a| a.get("runtime_dir"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .and_then(|p| canonicalize_path(&p).ok())
        .unwrap_or_else(|| current_runtime.clone());

    let installed_runtime = adapter_val
        .as_ref()
        .and_then(|a| a.get("runtime_dir"))
        .and_then(Value::as_str)
        .map(PathBuf::from);

    let runtime_matches = effective_runtime == current_runtime;

    let runtime_writable = fs::create_dir_all(&effective_runtime).is_ok()
        && (|| -> Result<bool> {
            let probe = effective_runtime.join(format!(".write-test-{}", uuid::Uuid::new_v4()));
            fs::write(&probe, b"ok")?;
            fs::remove_file(&probe)?;
            Ok(true)
        })()
        .unwrap_or(false);

    let current_states =
        canonicalize_path(&crate::fs_watcher::resolve_states_dir()).unwrap_or_default();
    let effective_states = adapter_val
        .as_ref()
        .and_then(|a| a.get("states_dir"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .and_then(|p| canonicalize_path(&p).ok())
        .unwrap_or_else(|| current_states.clone());

    let installed_states = adapter_val
        .as_ref()
        .and_then(|a| a.get("states_dir"))
        .and_then(Value::as_str)
        .map(PathBuf::from);

    let inventory_path_matches = effective_states == current_states;

    let mut snapshots = if let Some(ref req_sess) = args.session {
        crate::inventory::read_states(&effective_states)
            .into_iter()
            .filter(|s| {
                &s.name == req_sess
                    || s.inventory
                        .as_ref()
                        .and_then(|i| i.session_instance_id.as_ref())
                        .map(|sid| &sid.0)
                        == Some(req_sess)
            })
            .collect()
    } else {
        crate::inventory::read_states(&effective_states)
    };
    for s in &mut snapshots {
        crate::inventory::hydrate(s);
    }

    let inventory_sessions = crate::agent_setup::inventory_diagnostics(&snapshots);
    let inventory_ready = inventory_sessions.iter().any(|s| s["status"] == "ready");

    let mut discovery_disabled = false;
    let mut discovery_exclusion: Option<String> = None;
    let mut discovery_uncertainties = vec![
        "Launch-time --no-extensions CLI flag prevents autoloading regardless of installation."
            .to_string(),
        "Non-TUI modes (print, json, rpc) do not bind monitoring.".to_string(),
        "Session must run in interactive TUI within a verified Zellij terminal pane (ZELLIJ_PANE_ID set).".to_string(),
        "Project trust settings in un-trusted directories can suppress project extensions.".to_string(),
    ];

    // Inspect user global settings.json using exact upstream override semantics
    let user_settings_path = agent_dir.join("settings.json");
    if user_settings_path.exists() {
        if let Ok(Some(bytes)) = read_bounded_bytes(&user_settings_path, 32 * 1024) {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(exts) = val.get("extensions").and_then(Value::as_array) {
                    let (enabled, reason, uncerts) =
                        is_enabled_by_overrides(&ext_dir, &agent_dir, exts);
                    discovery_uncertainties.extend(uncerts);
                    if !enabled {
                        discovery_disabled = true;
                        discovery_exclusion = reason;
                    }
                }
            }
        }
    }

    // Inspect project settings .pi/settings.json (treated conservatively as uncertainty, not absolute blocker)
    if let Ok(cwd) = std::env::current_dir() {
        let proj_pi = cwd.join(".pi");
        let proj_settings = proj_pi.join("settings.json");
        if proj_settings.exists() {
            if let Ok(Some(bytes)) = read_bounded_bytes(&proj_settings, 32 * 1024) {
                if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(exts) = val.get("extensions").and_then(Value::as_array) {
                        for ext in exts {
                            if let Some(s) = ext.as_str() {
                                if s.starts_with('-') || s.starts_with('!') {
                                    discovery_uncertainties.push(format!(
                                        "Project settings (.pi/settings.json) contains extension override '{s}'; effective application depends on project trust"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Hydrated inventory & foreground process ownership verification
    let all_records = crate::agent_watcher::read_records(&effective_runtime.join("agents/v1"));
    let now_ms = crate::agent_store::now_ms();

    let mut record_details = Vec::new();
    let mut healthy_instances = 0;

    let requested_scope: Option<String> = if let Some(ref req_sess) = args.session {
        let matching_snap = snapshots.iter().find(|s| {
            &s.name == req_sess
                || s.inventory
                    .as_ref()
                    .and_then(|i| i.session_instance_id.as_ref())
                    .map(|sid| &sid.0)
                    == Some(req_sess)
        });
        matching_snap
            .and_then(|s| s.inventory.as_ref())
            .and_then(|i| i.session_instance_id.as_ref())
            .map(|sid| sid.0.clone())
            .or_else(|| Some(req_sess.clone()))
    } else {
        None
    };

    for r in all_records
        .iter()
        .filter(|r| r.identity.kind == verij_types::agent::AgentKind::Pi)
    {
        if let Some(ref req_scope) = requested_scope {
            if &r.identity.pane_key.session.0 != req_scope {
                continue;
            }
        }

        let is_alive = crate::process::is_alive(&r.identity.process);
        let is_foreground = !r.identity.is_synthetic
            && is_alive
            && snapshots.iter().any(|s| {
                s.inventory.as_ref().is_some_and(|inv| {
                    let session_match =
                        inv.session_instance_id.as_ref() == Some(&r.identity.pane_key.session);
                    session_match
                        && inv.server_process.as_ref().is_some_and(|server| {
                            inv.panes.iter().any(|p| {
                                p.terminal_id == r.identity.pane_key.terminal
                                    && !p.exited
                                    && p.pane_process.as_ref().is_some_and(|owner| {
                                        crate::process::verify_foreground(
                                            server,
                                            owner,
                                            &r.identity.process,
                                        )
                                        .is_ok()
                                    })
                            })
                        })
                })
            });

        let source_pi = r.state.sources.get("pi");
        let has_lease = source_pi.is_some_and(|s| s.capabilities.iter().any(|c| c == "lease"));
        let unexpired_lease = source_pi.is_some_and(|s| {
            now_ms.saturating_sub(s.observed_at_ms) <= crate::pi::LEASE_MS
                && s.observed_at_ms <= now_ms.saturating_add(1000)
        });
        let adapter_pi: Option<crate::pi::PiAdapterState> = r
            .state
            .adapter_state
            .get("pi")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let has_runtime = adapter_pi
            .as_ref()
            .is_some_and(|a| !a.runtime_id.is_empty());
        let running_ver = adapter_pi
            .as_ref()
            .map(|a| a.running_version.as_str())
            .unwrap_or("unknown");
        let version_supported = crate::pi::supports_version(running_ver);

        let epoch_matches = match (&source_pi, &adapter_pi) {
            (Some(s), Some(a)) => {
                a.source_epoch == s.source_epoch && a.source_revision == s.source_revision
            }
            _ => false,
        };

        let source_healthy =
            has_lease && unexpired_lease && has_runtime && version_supported && epoch_matches;
        if is_foreground && source_healthy {
            healthy_instances += 1;
        }

        record_details.push(json!({
            "instance": r.identity.agent_instance_id,
            "pid": r.identity.process.pid,
            "pane": r.identity.pane_key,
            "status": r.state.reduced.status,
            "is_foreground_owner": is_foreground,
            "source_healthy": source_healthy,
            "running_version": running_ver,
        }));
    }

    let adapter_loaded = healthy_instances > 0;

    let mut blockers = Vec::new();
    if !binary_available {
        if let Err(ref e) = ver_res {
            blockers.push(format!("Pi executable probe failed: {e}"));
        } else {
            blockers.push("Pi executable unavailable or failed to execute".to_string());
        }
    } else if !version_supported {
        blockers.push(format!(
            "Detected Pi version {:?} is not supported; qualified version is {}",
            ver, SUPPORTED_VERSION
        ));
    }
    if let Some(err) = manifest_err {
        blockers.push(format!("Manifest integrity check failed: {err}"));
    } else if !manifest_valid {
        blockers.push("Ownership manifest installed.json is missing or invalid".to_string());
    }
    if !exact_assets {
        blockers.push("Extension assets (index.ts, core.mjs) are missing or modified".to_string());
    }
    if !adapter_intact {
        blockers.push("Adapter configuration (adapter.json) is missing or modified".to_string());
    }
    if !reporter_available {
        blockers.push("Verij reporter executable is unavailable or invalid".to_string());
    }
    if !runtime_writable {
        blockers.push("Runtime directory is not writable".to_string());
    }
    if !runtime_matches {
        blockers
            .push("Current runtime directory differs from adapter.json installed path".to_string());
    }
    if symlink_detected {
        blockers.push(format!(
            "Symlinks detected in agent assets: {}",
            symlinks.join(", ")
        ));
    }
    if discovery_disabled {
        blockers.push(format!(
            "Discovery disabled by settings exclusion: {}",
            discovery_exclusion.as_deref().unwrap_or("unknown")
        ));
    }
    if !inventory_path_matches {
        blockers
            .push("Current inventory directory differs from installed adapter path".to_string());
    }
    if !inventory_ready {
        blockers.push(
            "No verified pane inventory is available for the selected session scope".to_string(),
        );
    }

    let installation_ready = binary_available
        && version_supported
        && manifest_valid
        && exact_assets
        && adapter_intact
        && reporter_available
        && !symlink_detected
        && !discovery_disabled;

    let monitoring_prerequisites_ready = installation_ready
        && inventory_ready
        && runtime_writable
        && runtime_matches
        && inventory_path_matches;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "adapter": "pi",
            "version": ver,
            "version_supported": version_supported,
            "supported_version": version_supported,
            "supported_versions": [SUPPORTED_VERSION],
            "binary_available": binary_available,
            "config_dir": agent_dir,
            "extensions_dir": ext_dir,
            "assets_intact": exact_assets,
            "index_intact": index_intact,
            "core_intact": core_intact,
            "adapter_intact": adapter_intact,
            "manifest_valid": manifest_valid,
            "reporter": effective_reporter,
            "reporter_available": reporter_available,
            "reporter_version": reporter_version,
            "runtime": effective_runtime,
            "runtime_writable": runtime_writable,
            "installed_runtime": installed_runtime,
            "runtime_matches_adapter": runtime_matches,
            "inventory_directory": current_states,
            "effective_inventory_directory": effective_states,
            "installed_inventory_directory": installed_states,
            "inventory_path_matches_adapter": inventory_path_matches,
            "inventory_sessions": inventory_sessions,
            "inventory_ready": inventory_ready,
            "requested_session": args.session,
            "discovery_disabled": discovery_disabled,
            "discovery_exclusion": discovery_exclusion,
            "discovery_uncertainties": discovery_uncertainties,
            "installation_ready": installation_ready,
            "structural_installation_ready": installation_ready,
            "adapter_loaded": adapter_loaded,
            "active_instances": healthy_instances,
            "monitoring_prerequisites_ready": monitoring_prerequisites_ready,
            "blockers": blockers,
            "live_records": record_details.len(),
            "records": record_details,
            "binding_requires": "Linux foreground tty + current native Zellij inventory",
            "capabilities": [
                "activity",
                "generic_blocking_ui",
                "conversation_title",
                "aggregate_completion",
                "terminal_failure"
            ],
            "limitations": [
                "Qualified strictly for Pi 1.2.0; other releases are unsupported.",
                "Non-interactive modes (print, json, rpc) do not bind monitoring.",
                "Conventional autoloading requires extensions/verij-monitoring; launch with --no-extensions or settings exclusions disables loading.",
                "Explicit local filesystem paths are required; file: URLs are not supported for configuration directories.",
                "Generic blocking UI dialogs (select, confirm, input, editor, custom) are observed, but universal permission provenance and assistant conversational questions are outside coverage.",
                "Extensions launching untracked background work remain outside aggregate-completion guarantees."
            ],
            "note": "Installation, runtime, and pane-inventory prerequisite checks. adapter_loaded confirms actual verified live reporting; structural installation alone is insufficient. Restart Pi after adapter setup/uninstall; reload the Verij exporter after a WASM upgrade."
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonicalize_nonexistent_and_parent_dir() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).unwrap();

        // 1. /root/missing/../agent => /root/agent
        let path = root.join("missing/../agent");
        let resolved = canonicalize_path(&path).unwrap();
        assert_eq!(resolved, root.join("agent"));

        // 2. /root/a/b/../../c/d => /root/c/d
        let path2 = root.join("a/b/../../c/d");
        let resolved2 = canonicalize_path(&path2).unwrap();
        assert_eq!(resolved2, root.join("c/d"));

        // 3. Existing symlink resolution
        let target = temp.path().join("symlink_target");
        fs::create_dir_all(&target).unwrap();
        let symlink_path = root.join("link");
        std::os::unix::fs::symlink(&target, &symlink_path).unwrap();

        let resolved_sym = canonicalize_path(&symlink_path.join("child")).unwrap();
        let canonical_target = target.canonicalize().unwrap();
        assert_eq!(resolved_sym, canonical_target.join("child"));
    }

    #[test]
    fn test_segment_glob_patterns() {
        assert!(!segment_glob("a?b", "a/b"));
        assert!(segment_glob("a?b", "acb"));

        assert!(!segment_glob("a*b", "a/b"));
        assert!(!segment_glob("a*b", "a/x/b"));
        assert!(segment_glob("a*b", "axxxb"));

        assert!(segment_glob(
            "**/verij-monitoring",
            "extensions/verij-monitoring"
        ));
        assert!(segment_glob("extensions/**", "extensions/verij-monitoring"));
        assert!(segment_glob("*verij-monitoring*", "verij-monitoring"));
        assert!(segment_glob(
            "**/*verij-monitoring*",
            "extensions/verij-monitoring"
        ));
    }

    #[test]
    fn test_is_enabled_by_overrides_exact_pi_contract() {
        let agent_dir = Path::new("/home/user/.pi/agent");
        let ext_dir = agent_dir.join("extensions/verij-monitoring");

        // 1. - exact exclude disables
        let (enabled, reason, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[json!("-extensions/verij-monitoring")],
        );
        assert!(!enabled);
        assert_eq!(reason.as_deref(), Some("-extensions/verij-monitoring"));

        // 2. - exact exclude runs AFTER + force include, so - WINS regardless of order!
        let (e1, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[
                json!("-extensions/verij-monitoring"),
                json!("+extensions/verij-monitoring"),
            ],
        );
        assert!(!e1, "Minus must win over plus");

        let (e2, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[
                json!("+extensions/verij-monitoring"),
                json!("-extensions/verij-monitoring"),
            ],
        );
        assert!(!e2, "Minus must win over plus in reverse order");

        // 3. + exact include re-enables what ! glob excluded
        let (e3, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[
                json!("!**/verij-monitoring"),
                json!("+extensions/verij-monitoring"),
            ],
        );
        assert!(e3, "Plus must re-enable what ! glob excluded");

        // 4. Minus with glob star is treated literally as string, NOT as glob!
        let (e4, _, _) = is_enabled_by_overrides(&ext_dir, agent_dir, &[json!("-extensions/*")]);
        assert!(
            e4,
            "Minus pattern with star is literal, does not glob-match"
        );

        // 5. Absolute minus exclusion disables discovery
        let abs_minus = format!("-{}", ext_dir.display());
        let (e5, r5, _) = is_enabled_by_overrides(&ext_dir, agent_dir, &[json!(abs_minus)]);
        assert!(!e5, "Absolute minus pattern must disable discovery");
        assert_eq!(r5.as_deref(), Some(abs_minus.as_str()));

        // 6. Absolute plus inclusion re-enables what ! glob excluded
        let abs_plus = format!("+{}", ext_dir.display());
        let (e6, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[json!("!**/verij-monitoring"), json!(abs_plus)],
        );
        assert!(e6, "Absolute plus pattern must re-enable discovery");

        // 7. Absolute minus wins over plus regardless of order
        let (e7, _, _) =
            is_enabled_by_overrides(&ext_dir, agent_dir, &[json!(abs_plus), json!(abs_minus)]);
        assert!(!e7, "Absolute minus must win over absolute plus");

        // 8. Relative ./ prefix in exact minus matches
        let (e8, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[json!("-./extensions/verij-monitoring")],
        );
        assert!(!e8, "Relative ./ prefix in minus pattern must match");

        // 9. Whitespace in pattern is NOT trimmed by Pi
        let (e9, _, _) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[json!("- extensions/verij-monitoring")],
        );
        assert!(
            e9,
            "Minus pattern with untrimmed space must not match path without space"
        );

        // 10. Unsupported bracket pattern flags uncertainty
        let (e10, _, uncerts10) = is_enabled_by_overrides(
            &ext_dir,
            agent_dir,
            &[json!("!extensions/[v]erij-monitoring")],
        );
        assert!(
            e10,
            "Unsupported bracket pattern must not disable discovery"
        );
        assert!(uncerts10
            .iter()
            .any(|u| u.contains("bracket") || u.contains("Complex glob")));
    }
}
