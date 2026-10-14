//! Native filesystem store for agent monitoring.
//!
//! Owns agent instance registration, source observation updates, atomic state replacement,
//! exclusive locking with fs2, and host-local completion acknowledgements.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use verij_types::agent::{
    reduce_sources, AgentIdentity, AgentInstanceId, AgentKind, AgentState, AgentStatus,
    HostAckStore, InstanceAck, PartialSourceRecord, ReducedExecutionState, VisitProof,
    AGENT_SCHEMA_VERSION,
};
use verij_types::identity::{valid_record_key, PaneKey, ProcessIdentity};

/// Current wall-clock time in milliseconds since UNIX epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve agent runtime directory.
///
/// Precedence:
/// 1. `VERIJ_AGENT_STATE_DIR` environment override (must be absolute)
/// 2. `$XDG_RUNTIME_DIR/verij` (must be absolute)
/// 3. Fallback: `/tmp/verij-<uid>`
pub fn resolve_agent_runtime_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("VERIJ_AGENT_STATE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if !path.is_absolute() {
                bail!("VERIJ_AGENT_STATE_DIR must be absolute: {:?}", path);
            }
            crate::process::ensure_private_directory(&path)?;
            return Ok(path);
        }
    }

    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let trimmed = runtime_dir.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed).join("verij");
            if path.is_absolute() && crate::process::ensure_private_directory(&path).is_ok() {
                return Ok(path);
            }
        }
    }

    let uid = unsafe { libc::getuid() };
    let path = PathBuf::from(format!("/tmp/verij-{}", uid));
    crate::process::ensure_private_directory(&path)?;
    Ok(path)
}

/// Resolve `<runtime>/agents/v1/` directory.
pub fn resolve_v1_dir() -> Result<PathBuf> {
    let runtime_dir = resolve_agent_runtime_dir()?;
    let v1_dir = runtime_dir.join("agents").join("v1");
    crate::process::ensure_private_directory(&v1_dir)?;
    Ok(v1_dir)
}

/// Resolve `<verij-state>/agent-monitoring/hosts/` directory.
pub fn resolve_hosts_dir() -> Result<PathBuf> {
    let state_dir = crate::config::state_dir()
        .or_else(|| {
            std::env::var("XDG_STATE_HOME")
                .ok()
                .map(|p| PathBuf::from(p).join("verij"))
        })
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|p| PathBuf::from(p).join(".local/state/verij"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/verij-state"));
    let hosts_dir = state_dir.join("agent-monitoring").join("hosts");
    crate::process::ensure_private_directory(&hosts_dir)?;
    Ok(hosts_dir)
}

/// RAII lock guard for per-instance mutual exclusion.
pub struct InstanceLock {
    file: File,
    _path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        let lock_path = dir.join(".lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open instance lock at {:?}", lock_path))?;
        file.lock_exclusive()
            .with_context(|| format!("failed to acquire exclusive lock on {:?}", lock_path))?;
        Ok(Self {
            file,
            _path: lock_path,
        })
    }

    pub fn try_acquire(dir: &Path) -> Result<Option<Self>> {
        let lock_path = dir.join(".lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open instance lock at {:?}", lock_path))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self {
                file,
                _path: lock_path,
            })),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.raw_os_error() == Some(libc::EWOULDBLOCK) =>
            {
                Ok(None)
            }
            Err(err) => Err(err)
                .with_context(|| format!("failed to acquire exclusive lock on {:?}", lock_path)),
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// RAII lock guard for global registration and pruning serialization.
pub struct RegistrationLock {
    file: File,
    _path: PathBuf,
}

impl RegistrationLock {
    pub fn acquire(v1_dir: &Path) -> Result<Self> {
        let _ = crate::process::ensure_private_directory(v1_dir);
        let lock_path = v1_dir.join(".registration.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open registration lock at {:?}", lock_path))?;
        file.lock_exclusive()
            .with_context(|| format!("failed to acquire registration lock on {:?}", lock_path))?;
        Ok(Self {
            file,
            _path: lock_path,
        })
    }

    pub fn try_acquire(v1_dir: &Path) -> Result<Option<Self>> {
        let _ = crate::process::ensure_private_directory(v1_dir);
        let lock_path = v1_dir.join(".registration.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open registration lock at {:?}", lock_path))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self {
                file,
                _path: lock_path,
            })),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.raw_os_error() == Some(libc::EWOULDBLOCK) =>
            {
                Ok(None)
            }
            Err(err) => Err(err)
                .with_context(|| format!("failed to acquire registration lock on {:?}", lock_path)),
        }
    }
}

impl Drop for RegistrationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// RAII lock guard for host-local acknowledgement serialization.
pub struct HostLock {
    file: File,
    _path: PathBuf,
}

impl HostLock {
    pub fn acquire(hosts_dir: &Path, host_key: &str) -> Result<Self> {
        let lock_path = hosts_dir.join(format!("{}.lock", host_key));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open host lock at {:?}", lock_path))?;
        file.lock_exclusive()
            .with_context(|| format!("failed to acquire host lock on {:?}", lock_path))?;
        Ok(Self {
            file,
            _path: lock_path,
        })
    }
}

impl Drop for HostLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub dev: u64,
    pub ino: u64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
    pub len: u64,
}

impl FileStamp {
    pub fn from_metadata(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: meta.dev(),
                ino: meta.ino(),
                mtime_sec: meta.mtime(),
                mtime_nsec: meta.mtime_nsec(),
                ctime_sec: meta.ctime(),
                ctime_nsec: meta.ctime_nsec(),
                len: meta.len(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                dev: 0,
                ino: 0,
                mtime_sec: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64),
                mtime_nsec: 0,
                ctime_sec: 0,
                ctime_nsec: 0,
                len: meta.len(),
            }
        }
    }
}

pub const IDENTITY_MAX_BYTES: usize = 64 * 1024;
pub const STATE_MAX_BYTES: usize = 256 * 1024;
pub const HOST_ACK_MAX_BYTES: usize = 256 * 1024;

/// Read at most `max_bytes + 1` bytes before allocation / JSON parsing.
/// Returns the read bytes alongside the exact file descriptor metadata from fstat.
/// Uses O_NONBLOCK and checks is_file() so opening FIFOs or special files cannot block.
pub fn read_bounded_bytes_with_meta(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, fs::Metadata)> {
    use std::io::Read;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {:?}", path))?;

    let metadata = file
        .metadata()
        .with_context(|| format!("failed to stat {:?}", path))?;
    if !metadata.is_file() {
        bail!("file {:?} is not a regular file", path);
    }

    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {:?}", path))?;
    if bytes.len() > max_bytes {
        bail!("file {:?} exceeds size limit of {} bytes", path, max_bytes);
    }
    Ok((bytes, metadata))
}

pub fn read_bounded_bytes(path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    read_bounded_bytes_with_meta(path, max_bytes).map(|(bytes, _)| bytes)
}

#[allow(dead_code)]
pub fn read_bounded_string(path: &Path, max_bytes: usize) -> Result<String> {
    let bytes = read_bounded_bytes(path, max_bytes)?;
    let text =
        String::from_utf8(bytes).with_context(|| format!("file {:?} is not valid UTF-8", path))?;
    Ok(text)
}

pub fn read_bounded_json_with_meta<T: serde::de::DeserializeOwned>(
    path: &Path,
    max_bytes: usize,
) -> Result<(T, fs::Metadata)> {
    let (bytes, meta) = read_bounded_bytes_with_meta(path, max_bytes)?;
    let val = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse JSON from {:?}", path))?;
    Ok((val, meta))
}

pub fn read_bounded_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    max_bytes: usize,
) -> Result<T> {
    read_bounded_json_with_meta(path, max_bytes).map(|(val, _)| val)
}

/// Unique same-directory temp atomic replace + fsync.
pub fn atomic_write_json<T: Serialize>(target_path: &Path, value: &T) -> Result<()> {
    let parent = target_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent directory for {:?}", target_path))?;
    let file_name = target_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("file");
    let temp_name = format!(".{}.tmp.{}", file_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(temp_name);

    let bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("failed to serialize JSON for {:?}", target_path))?;
    if bytes.len() > 256 * 1024 {
        bail!("monitoring record exceeds 256 KiB limit");
    }

    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("failed to create temp file {:?}", temp_path))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }

        file.write_all(&bytes)?;
        file.sync_all()
            .with_context(|| format!("fsync failed on {:?}", temp_path))?;
    }

    fs::rename(&temp_path, target_path).with_context(|| {
        format!(
            "failed to atomic rename {:?} to {:?}",
            temp_path, target_path
        )
    })?;
    crate::resource_metrics::record_write();

    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Register a monitored agent instance for a verified pane and process.
///
/// Repeated registration for the exact same verified process and pane returns the same ID.
/// Conflicts on immutable attributes (kind, synthetic, runner) are strictly rejected.
pub fn register(
    pane_key: PaneKey,
    kind: AgentKind,
    process: ProcessIdentity,
    is_synthetic: bool,
    runner_id: Option<String>,
) -> Result<AgentInstanceId> {
    register_inner(pane_key, kind, process, is_synthetic, runner_id, false)
        .map(|opt| opt.expect("blocking register"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    Pending,
    Complete(AgentInstanceId),
}

/// Owned resumable cursor for bounded background agent registration.
/// Scans <=32 entries per slice under try-acquired registration lock.
/// Concurrent directory changes invalidate the cursor and trigger a conservative restart.
/// At EOF under the same registration lock, allocates the new instance atomically.
pub struct RegistrationCursor {
    pane_key: PaneKey,
    kind: AgentKind,
    process: ProcessIdentity,
    is_synthetic: bool,
    runner_id: Option<String>,
    v1_dir: PathBuf,
    read_dir: Option<fs::ReadDir>,
    dir_stamp: Option<FileStamp>,
    outcome: Option<AgentInstanceId>,
}

impl RegistrationCursor {
    pub fn new(
        pane_key: PaneKey,
        kind: AgentKind,
        process: ProcessIdentity,
        is_synthetic: bool,
        runner_id: Option<String>,
    ) -> Result<Self> {
        if !is_synthetic && !crate::process::is_alive(&process) {
            bail!("cannot register dead process {}", process.pid);
        }
        let v1_dir = resolve_v1_dir()?;
        Ok(Self {
            pane_key,
            kind,
            process,
            is_synthetic,
            runner_id,
            v1_dir,
            read_dir: None,
            dir_stamp: None,
            outcome: None,
        })
    }

    pub fn step(
        &mut self,
        stop: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<RegistrationOutcome> {
        if let Some(id) = self.outcome.as_ref() {
            return Ok(RegistrationOutcome::Complete(id.clone()));
        }
        if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
            return Ok(RegistrationOutcome::Pending);
        }
        if !self.is_synthetic && !crate::process::is_alive(&self.process) {
            bail!("cannot register dead process {}", self.process.pid);
        }

        let _reg_lock = match RegistrationLock::try_acquire(&self.v1_dir)? {
            Some(l) => l,
            None => return Ok(RegistrationOutcome::Pending),
        };

        let current_meta = fs::metadata(&self.v1_dir)?;
        let current_stamp = FileStamp::from_metadata(&current_meta);

        if self.dir_stamp.as_ref() != Some(&current_stamp) || self.read_dir.is_none() {
            self.read_dir = Some(fs::read_dir(&self.v1_dir)?);
            self.dir_stamp = Some(current_stamp);
        }

        let read_dir = self.read_dir.as_mut().unwrap();
        let mut count = 0;
        while count < 32 {
            if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
                return Ok(RegistrationOutcome::Pending);
            }
            match read_dir.next() {
                Some(Ok(entry)) => {
                    count += 1;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if valid_record_key(n) => n,
                        _ => continue,
                    };
                    if path.join(".tombstone").exists() {
                        continue;
                    }
                    let identity_file = path.join("identity.json");
                    let state_file = path.join("state.json");
                    if identity_file.exists() && state_file.exists() {
                        if let Ok(existing) =
                            read_bounded_json::<AgentIdentity>(&identity_file, IDENTITY_MAX_BYTES)
                        {
                            if existing.schema_version != AGENT_SCHEMA_VERSION {
                                continue;
                            }
                            if existing.agent_instance_id.0 != name {
                                continue;
                            }
                            if existing.pane_key == self.pane_key
                                && existing.process == self.process
                            {
                                if existing.kind != self.kind
                                    || existing.is_synthetic != self.is_synthetic
                                    || existing.runner_id != self.runner_id
                                {
                                    bail!(
                                        "registration conflict for pane {:?} and pid {}: existing (kind={:?}, synthetic={}, runner={:?}) != requested (kind={:?}, synthetic={}, runner={:?})",
                                        self.pane_key,
                                        self.process.pid,
                                        existing.kind,
                                        existing.is_synthetic,
                                        existing.runner_id,
                                        self.kind,
                                        self.is_synthetic,
                                        self.runner_id
                                    );
                                }
                                self.outcome = Some(existing.agent_instance_id.clone());
                                return Ok(RegistrationOutcome::Complete(
                                    existing.agent_instance_id,
                                ));
                            }
                        }
                    }
                }
                Some(Err(_)) => {
                    count += 1;
                    continue;
                }
                None => {
                    let new_id = allocate_instance_under_lock(
                        &self.v1_dir,
                        self.pane_key.clone(),
                        self.kind,
                        self.process.clone(),
                        self.is_synthetic,
                        self.runner_id.clone(),
                        true,
                    )?;
                    let Some(new_id) = new_id else {
                        return Ok(RegistrationOutcome::Pending);
                    };
                    self.outcome = Some(new_id.clone());
                    return Ok(RegistrationOutcome::Complete(new_id));
                }
            }
        }

        Ok(RegistrationOutcome::Pending)
    }
}

fn allocate_instance_under_lock(
    v1_dir: &Path,
    pane_key: PaneKey,
    kind: AgentKind,
    process: ProcessIdentity,
    is_synthetic: bool,
    runner_id: Option<String>,
    nonblocking: bool,
) -> Result<Option<AgentInstanceId>> {
    let new_id = format!("agent-{}", uuid::Uuid::new_v4());
    if !valid_record_key(&new_id) {
        bail!("generated instance id {:?} failed validation", new_id);
    }
    let instance_id = AgentInstanceId(new_id);
    let instance_dir = v1_dir.join(&instance_id.0);
    crate::process::ensure_private_directory(&instance_dir)?;

    let _inst_lock = if nonblocking {
        match InstanceLock::try_acquire(&instance_dir)? {
            Some(l) => l,
            None => return Ok(None),
        }
    } else {
        InstanceLock::acquire(&instance_dir)?
    };

    let identity = AgentIdentity {
        schema_version: AGENT_SCHEMA_VERSION,
        agent_instance_id: instance_id.clone(),
        pane_key,
        kind,
        process,
        registered_at_ms: now_ms(),
        is_synthetic,
        runner_id,
    };
    atomic_write_json(&instance_dir.join("identity.json"), &identity)?;

    let initial_reduced = ReducedExecutionState {
        status: AgentStatus::Unknown,
        current_turn_id: None,
        current_turn_epoch: 0,
        current_turn_revision: 0,
        conversation_id: None,
        conversation_title: None,
        pending_requests: Vec::new(),
        background_task_count: 0,
        latest_error: None,
        latest_completion: None,
        completion_revision: 0,
        detail: None,
    };

    let initial_state = AgentState {
        schema_version: AGENT_SCHEMA_VERSION,
        agent_instance_id: instance_id.clone(),
        record_revision: 1,
        updated_at_ms: now_ms(),
        sources: BTreeMap::new(),
        reduced: initial_reduced,
        completion_revision: 0,
        completed_turn: None,
        completed_turn_epoch: 0,
        completed_turn_revision: 0,
        opencode_completions: BTreeMap::new(),
        adapter_state: BTreeMap::new(),
    };
    atomic_write_json(&instance_dir.join("state.json"), &initial_state)?;

    Ok(Some(instance_id))
}

fn register_inner(
    pane_key: PaneKey,
    kind: AgentKind,
    process: ProcessIdentity,
    is_synthetic: bool,
    runner_id: Option<String>,
    nonblocking: bool,
) -> Result<Option<AgentInstanceId>> {
    if !is_synthetic && !crate::process::is_alive(&process) {
        bail!("cannot register dead process {}", process.pid);
    }

    let v1_dir = resolve_v1_dir()?;
    let _reg_lock = if nonblocking {
        match RegistrationLock::try_acquire(&v1_dir)? {
            Some(l) => l,
            None => return Ok(None),
        }
    } else {
        RegistrationLock::acquire(&v1_dir)?
    };

    // Scan existing instances for matching pane_key and process lifetime
    if let Ok(entries) = fs::read_dir(&v1_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) if valid_record_key(n) => n,
                _ => continue,
            };
            if path.join(".tombstone").exists() {
                continue;
            }

            let identity_file = path.join("identity.json");
            let state_file = path.join("state.json");

            // Readers ignore incomplete registration without state.json
            if identity_file.exists() && state_file.exists() {
                if let Ok(existing) =
                    read_bounded_json::<AgentIdentity>(&identity_file, IDENTITY_MAX_BYTES)
                {
                    if existing.schema_version != AGENT_SCHEMA_VERSION {
                        continue;
                    }
                    if existing.agent_instance_id.0 != name {
                        continue;
                    }
                    if existing.pane_key == pane_key && existing.process == process {
                        // Enforce immutable attribute match: kind, synthetic, runner_id
                        if existing.kind != kind
                            || existing.is_synthetic != is_synthetic
                            || existing.runner_id != runner_id
                        {
                            bail!(
                                "registration conflict for pane {:?} and pid {}: existing (kind={:?}, synthetic={}, runner={:?}) != requested (kind={:?}, synthetic={}, runner={:?})",
                                pane_key,
                                process.pid,
                                existing.kind,
                                existing.is_synthetic,
                                existing.runner_id,
                                kind,
                                is_synthetic,
                                runner_id
                            );
                        }
                        return Ok(Some(existing.agent_instance_id));
                    }
                }
            }
        }
    }

    allocate_instance_under_lock(
        &v1_dir,
        pane_key,
        kind,
        process,
        is_synthetic,
        runner_id,
        nonblocking,
    )
}

/// Report source observation, atomically updating `state.json`.
pub fn report(instance_id: &AgentInstanceId, partial: PartialSourceRecord) -> Result<AgentState> {
    update(instance_id, |_| Ok(partial), |_| Ok(()))
}

/// Adapter projection and completion bookkeeping share the record transaction.
pub fn update(
    instance_id: &AgentInstanceId,
    project: impl FnOnce(&mut AgentState) -> Result<PartialSourceRecord>,
    finish: impl FnOnce(&mut AgentState) -> Result<()>,
) -> Result<AgentState> {
    update_inner(instance_id, project, finish, false).map(|opt| opt.expect("blocking update"))
}

pub fn update_try(
    instance_id: &AgentInstanceId,
    project: impl FnOnce(&mut AgentState) -> Result<PartialSourceRecord>,
    finish: impl FnOnce(&mut AgentState) -> Result<()>,
) -> Result<Option<AgentState>> {
    update_inner(instance_id, project, finish, true)
}

fn update_inner(
    instance_id: &AgentInstanceId,
    project: impl FnOnce(&mut AgentState) -> Result<PartialSourceRecord>,
    finish: impl FnOnce(&mut AgentState) -> Result<()>,
    nonblocking: bool,
) -> Result<Option<AgentState>> {
    if !valid_record_key(&instance_id.0) {
        bail!("invalid agent instance id {:?}", instance_id.0);
    }

    let v1_dir = resolve_v1_dir()?;
    let instance_dir = v1_dir.join(&instance_id.0);
    let _registration = if nonblocking {
        match RegistrationLock::try_acquire(&v1_dir)? {
            Some(l) => l,
            None => return Ok(None),
        }
    } else {
        RegistrationLock::acquire(&v1_dir)?
    };
    if !instance_dir.exists() {
        bail!(
            "agent instance {:?} directory does not exist",
            instance_id.0
        );
    }

    let _inst_lock = if nonblocking {
        match InstanceLock::try_acquire(&instance_dir)? {
            Some(l) => l,
            None => return Ok(None),
        }
    } else {
        InstanceLock::acquire(&instance_dir)?
    };

    // Recheck existence under lock to prevent ghost revival after prune
    if instance_dir.join(".tombstone").exists()
        || !instance_dir.join("identity.json").exists()
        || !instance_dir.join("state.json").exists()
    {
        bail!("agent instance {:?} is pruned or incomplete", instance_id.0);
    }

    let id_file = instance_dir.join("identity.json");
    let identity: AgentIdentity = read_bounded_json(&id_file, IDENTITY_MAX_BYTES)?;

    if identity.schema_version != AGENT_SCHEMA_VERSION {
        bail!(
            "unsupported identity schema_version {}; expected {}",
            identity.schema_version,
            AGENT_SCHEMA_VERSION
        );
    }
    if identity.agent_instance_id != *instance_id {
        bail!(
            "identity agent_instance_id mismatch: {:?} != {:?}",
            identity.agent_instance_id,
            instance_id
        );
    }

    // Validate live process birth (synthetic path explicit only)
    if identity.is_synthetic {
        if !Path::new(&format!("/proc/{}", identity.process.pid)).exists() {
            bail!(
                "cannot report on dead synthetic process {}",
                identity.process.pid
            );
        }
    } else if !crate::process::is_alive(&identity.process) {
        bail!("cannot report on dead process {}", identity.process.pid);
    }

    let state_file = instance_dir.join("state.json");
    let mut state: AgentState = read_bounded_json(&state_file, STATE_MAX_BYTES)?;

    if state.schema_version != AGENT_SCHEMA_VERSION {
        bail!(
            "unsupported state schema_version {}; expected {}",
            state.schema_version,
            AGENT_SCHEMA_VERSION
        );
    }
    if state.agent_instance_id != *instance_id {
        bail!(
            "state agent_instance_id mismatch: {:?} != {:?}",
            state.agent_instance_id,
            instance_id
        );
    }

    let partial = project(&mut state)?;

    // Enforce bounds on source count and payload fields
    if state.sources.len() >= 64 && !state.sources.contains_key(&partial.source_id) {
        bail!(
            "maximum source count limit (64) reached for instance {:?}",
            instance_id.0
        );
    }
    if let Some(ref reqs) = partial.pending_requests {
        if reqs.len() > 100 {
            bail!("too many pending requests in report (max 100)");
        }
    }
    if let Some(ref caps) = partial.capabilities {
        if caps.len() > 100 {
            bail!("too many capabilities in report (max 100)");
        }
    }
    if let Some(ref title) = partial.conversation_title {
        if title.len() > 1024 {
            bail!("conversation title exceeds maximum length (1024)");
        }
    }
    if let Some(ref err) = partial.execution_error {
        if err.message.len() > 4096 {
            bail!("execution error message exceeds maximum length (4096)");
        }
    }
    if let Some(ref succ) = partial.successful_completion {
        if let Some(ref sum) = succ.summary {
            if sum.len() > 4096 {
                bail!("successful completion summary exceeds maximum length (4096)");
            }
        }
    }

    let existing_src = state.sources.get(&partial.source_id);
    let updated_src = partial
        .merge_into(instance_id, existing_src, now_ms())
        .map_err(|e| anyhow::anyhow!(e))?;

    state.sources.insert(partial.source_id.clone(), updated_src);

    let (reduced, comp_rev, comp_turn, comp_epoch, comp_rev_num) = reduce_sources(
        &state.sources,
        identity.kind,
        identity.is_synthetic,
        state.completion_revision,
        state.completed_turn.as_ref(),
        state.completed_turn_epoch,
        state.completed_turn_revision,
    );

    state.reduced = reduced;
    state.completion_revision = comp_rev;
    state.completed_turn = comp_turn;
    state.completed_turn_epoch = comp_epoch;
    state.completed_turn_revision = comp_rev_num;
    state.record_revision += 1;
    state.updated_at_ms = now_ms();

    finish(&mut state)?;

    atomic_write_json(&state_file, &state)?;

    Ok(Some(state))
}

/// Inspect agent identity and reduced state.
pub fn inspect(instance_id: &AgentInstanceId) -> Result<(AgentIdentity, AgentState)> {
    if !valid_record_key(&instance_id.0) {
        bail!("invalid agent instance id {:?}", instance_id.0);
    }

    let v1_dir = resolve_v1_dir()?;
    let instance_dir = v1_dir.join(&instance_id.0);
    if !instance_dir.exists() {
        bail!(
            "agent instance {:?} directory does not exist",
            instance_id.0
        );
    }

    let _inst_lock = InstanceLock::acquire(&instance_dir)?;

    if instance_dir.join(".tombstone").exists() {
        bail!("agent instance {:?} was pruned", instance_id.0);
    }

    let identity_file = instance_dir.join("identity.json");
    let state_file = instance_dir.join("state.json");

    if !identity_file.exists() || !state_file.exists() {
        bail!(
            "agent instance {:?} has incomplete registration",
            instance_id.0
        );
    }

    let identity: AgentIdentity = read_bounded_json(&identity_file, IDENTITY_MAX_BYTES)?;
    let state: AgentState = read_bounded_json(&state_file, STATE_MAX_BYTES)?;

    Ok((identity, state))
}

/// Acknowledge a visited completion for a specific host.
///
/// Requires typed VisitProof with verified whole-host and inner focus.
/// Proof age must be fresh (<= 2 seconds), future-bounded, with valid request ID and live process.
/// Never decreases acknowledgement revision and never consumes newer completions.
/// Fails closed on corrupt host ack store.
#[cfg(test)]
pub fn acknowledge(proof: &VisitProof) -> Result<InstanceAck> {
    acknowledge_if_current(proof, || true)
}

/// Recheck local intent after acquiring persistence locks and immediately
/// before publication. A superseded worker must not acknowledge after waiting
/// behind another reporter or host writer.
pub fn acknowledge_if_current(
    proof: &VisitProof,
    is_current: impl Fn() -> bool,
) -> Result<InstanceAck> {
    acknowledge_checked(proof, None, is_current)
}

/// Passive work is qualified by the exact current identity and displayed completion,
/// checked under the instance lock. Explicit navigation retains older-revision semantics.
pub(crate) fn acknowledge_current_if_current(
    proof: &VisitProof,
    identity: &verij_types::agent::AgentIdentity,
    is_current: impl Fn() -> bool,
) -> Result<InstanceAck> {
    acknowledge_checked(proof, Some(identity), is_current)
}

fn acknowledge_checked(
    proof: &VisitProof,
    expected_identity: Option<&verij_types::agent::AgentIdentity>,
    is_current: impl Fn() -> bool,
) -> Result<InstanceAck> {
    if !is_current() {
        bail!("visit superseded locally before persistence");
    }
    if !proof.whole_host_focused {
        bail!("refusing unverified visit proof: whole_host_focused is false");
    }
    if !proof.inner_focus_verified {
        bail!("refusing unverified visit proof: inner_focus_verified is false");
    }
    if proof.observed_completion_revision == 0 {
        bail!("refusing visit proof with completion revision 0");
    }
    if proof.request_id.trim().is_empty() || !valid_record_key(&proof.request_id) {
        bail!(
            "refusing visit proof with invalid or empty request_id {:?}",
            proof.request_id
        );
    }
    if !valid_record_key(&proof.host_key.0) {
        bail!("invalid host_key {:?}", proof.host_key.0);
    }
    if !valid_record_key(&proof.agent_instance_id.0) {
        bail!("invalid agent_instance_id {:?}", proof.agent_instance_id.0);
    }

    let now = now_ms();
    // Fresh bounded future: cannot be more than 1 second in future
    if proof.verified_at_ms > now + 1000 {
        bail!(
            "refusing visit proof in the future: verified_at_ms {} > now {}",
            proof.verified_at_ms,
            now
        );
    }
    // Fresh bounded past: age must be <= 2 seconds (2000 ms)
    let age = now.saturating_sub(proof.verified_at_ms);
    if age > 2000 {
        bail!("refusing stale visit proof: age {}ms > 2000ms", age);
    }

    let v1_dir = resolve_v1_dir()?;
    let instance_dir = v1_dir.join(&proof.agent_instance_id.0);
    if !instance_dir.exists() || instance_dir.join(".tombstone").exists() {
        bail!(
            "agent instance {:?} directory does not exist or is pruned",
            proof.agent_instance_id.0
        );
    }

    // Verify instance binding and observed revision under instance lock
    let _inst_lock = InstanceLock::acquire(&instance_dir)?;
    let id_file = instance_dir.join("identity.json");
    let identity: AgentIdentity = read_bounded_json(&id_file, IDENTITY_MAX_BYTES)?;
    if expected_identity.is_some_and(|expected| expected != &identity) {
        bail!("passive visit target identity changed");
    }

    if identity.pane_key != proof.pane_key {
        bail!(
            "pane key mismatch: identity {:?} vs proof {:?}",
            identity.pane_key,
            proof.pane_key
        );
    }
    if identity.agent_instance_id != proof.agent_instance_id {
        bail!(
            "instance ID mismatch: identity {:?} vs proof {:?}",
            identity.agent_instance_id,
            proof.agent_instance_id
        );
    }

    // Live process verification
    if identity.is_synthetic {
        if !Path::new(&format!("/proc/{}", identity.process.pid)).exists() {
            bail!(
                "cannot acknowledge for dead synthetic process {}",
                identity.process.pid
            );
        }
    } else if !crate::process::is_alive(&identity.process) {
        bail!(
            "cannot acknowledge for dead process {}",
            identity.process.pid
        );
    }

    let state_file = instance_dir.join("state.json");
    let state: AgentState = read_bounded_json(&state_file, STATE_MAX_BYTES)?;
    if expected_identity.is_some()
        && (state.reduced.status != AgentStatus::Done
            || state.reduced.completion_revision != proof.observed_completion_revision)
    {
        bail!("passive visit completion changed");
    }

    if crate::opencode::lease_expired(&state, now_ms()) {
        bail!("OpenCode reporter lease expired");
    }
    if state.completion_revision == 0 {
        bail!("refusing acknowledgement: agent has no completed turns (completion_revision is 0)");
    }

    if proof.observed_completion_revision > state.completion_revision {
        bail!(
            "observed completion revision {} exceeds current state completion revision {}",
            proof.observed_completion_revision,
            state.completion_revision
        );
    }
    // Hold the instance lock through host persistence so pruning cannot remove
    // this identity midway through acknowledging its observed revision.

    // Host-local persistence under host lock
    let hosts_dir = resolve_hosts_dir()?;
    if !is_current() {
        bail!("visit superseded locally before host persistence lock");
    }
    let _host_lock = HostLock::acquire(&hosts_dir, &proof.host_key.0)?;
    if !is_current() {
        bail!("visit superseded locally while waiting for persistence");
    }
    if now_ms().saturating_sub(proof.verified_at_ms) > 2000 {
        bail!("visit proof expired while waiting for persistence");
    }
    let host_file = hosts_dir.join(format!("{}.json", proof.host_key.0));

    // Corrupt host ack store must FAIL CLOSED, never silently reset
    let mut host_store: HostAckStore = if host_file.exists() {
        let store: HostAckStore =
            read_bounded_json(&host_file, HOST_ACK_MAX_BYTES).with_context(|| {
                format!("corrupt host ack store at {:?}, failing closed", host_file)
            })?;
        if store.schema_version != AGENT_SCHEMA_VERSION {
            bail!(
                "unsupported host ack store schema version {} at {:?}",
                store.schema_version,
                host_file
            );
        }
        if store.host_key != proof.host_key {
            bail!(
                "host key mismatch in store: {:?} != {:?}",
                store.host_key,
                proof.host_key
            );
        }
        store
    } else {
        HostAckStore::new(proof.host_key.clone(), now)
    };

    // Never decrease acknowledgement revision
    if let Some(existing) = host_store.acknowledgements.get(&proof.agent_instance_id.0) {
        if existing.acknowledged_revision >= proof.observed_completion_revision {
            return Ok(existing.clone());
        }
    }

    let ack_turn = if proof.observed_completion_revision == state.completion_revision {
        state.completed_turn.clone()
    } else {
        None
    };

    let ack = InstanceAck {
        acknowledged_revision: proof.observed_completion_revision,
        acknowledged_at_ms: now,
        turn_id: ack_turn,
        request_id: proof.request_id.clone(),
    };

    host_store
        .acknowledgements
        .insert(proof.agent_instance_id.0.clone(), ack.clone());
    host_store.updated_at_ms = now;

    if !is_current() {
        bail!("visit superseded locally before publication");
    }
    atomic_write_json(&host_file, &host_store)?;

    Ok(ack)
}

/// Prune dead agent records whose processes have exited.
///
/// Acquires registration lock then instance lock in strict order.
/// Writers acquiring an instance recheck existence and tombstone under lock.
pub fn prune(dry_run: bool) -> Result<Vec<AgentInstanceId>> {
    let v1_dir = resolve_v1_dir()?;
    let mut cursor = None;
    let mut pruned = Vec::new();
    loop {
        let chunk = prune_slice_in_root(&v1_dir, &mut cursor, 64, dry_run, None)?;
        pruned.extend(chunk);
        if cursor.is_none() {
            break;
        }
    }
    Ok(pruned)
}

/// Sliced, cancellable, resumable dead-record pruning using a persistent ReadDir cursor.
/// Counts ALL entries including invalid/skipped ones.
/// Checks stop between entries.
/// Nonblocking locks: skips without blocking if locks are held.
/// Uses is_definitely_dead to ensure uncertain records are never pruned.
/// Safely removes only expected files in instance directory.
pub fn prune_slice_in_root(
    v1_dir: &Path,
    cursor: &mut Option<fs::ReadDir>,
    slice_limit: usize,
    dry_run: bool,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<AgentInstanceId>> {
    let _reg_lock = match RegistrationLock::try_acquire(v1_dir)? {
        Some(l) => l,
        None => return Ok(Vec::new()),
    };

    if cursor.is_none() {
        *cursor = fs::read_dir(v1_dir).ok();
    }
    let Some(read_dir) = cursor.as_mut() else {
        return Ok(Vec::new());
    };

    let mut pruned = Vec::new();
    let mut inspected = 0;

    while inspected < slice_limit {
        if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
            break;
        }

        let entry = match read_dir.next() {
            Some(Ok(e)) => {
                inspected += 1;
                e
            }
            Some(Err(_)) => {
                inspected += 1;
                continue;
            }
            None => {
                *cursor = None;
                break;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name_str) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !valid_record_key(name_str) {
            continue;
        }

        let id = AgentInstanceId(name_str.to_string());
        let identity_file = path.join("identity.json");
        if !identity_file.exists() {
            continue;
        }

        let _inst_lock = match InstanceLock::try_acquire(&path)? {
            Some(lock) => lock,
            None => continue,
        };

        if path.join(".tombstone").exists() {
            continue;
        }

        let identity: AgentIdentity = match read_bounded_json(&identity_file, IDENTITY_MAX_BYTES) {
            Ok(id) => id,
            Err(_) => continue,
        };

        if identity.schema_version != AGENT_SCHEMA_VERSION || identity.agent_instance_id != id {
            continue;
        }

        // Non-synthetic liveness checking requires definitive death.
        // Permission errors or uncertain process states are never treated as death.
        let is_dead = if identity.is_synthetic {
            !Path::new(&format!("/proc/{}", identity.process.pid)).exists()
        } else {
            crate::process::is_definitely_dead(&identity.process)
        };

        if !is_dead {
            continue;
        }

        if !dry_run {
            // Write tombstone first under instance lock to alert waiting writers
            let _ = fs::write(path.join(".tombstone"), b"pruned");
            let mut unexpected_files = false;
            if let Ok(dir_entries) = fs::read_dir(&path) {
                for item in dir_entries.flatten() {
                    let file_name = item.file_name();
                    let name = file_name.to_string_lossy();
                    if name != "identity.json"
                        && name != "state.json"
                        && name != ".lock"
                        && name != ".tombstone"
                    {
                        unexpected_files = true;
                        break;
                    }
                }
            }

            // Always remove known state data
            let _ = fs::remove_file(path.join("identity.json"));
            let _ = fs::remove_file(path.join("state.json"));

            if !unexpected_files {
                let _ = fs::remove_file(path.join(".tombstone"));
                drop(_inst_lock);
                let _ = fs::remove_file(path.join(".lock"));
                let _ = fs::remove_dir(&path);
            } else {
                // Keep .tombstone and .lock to prevent lock splitting when unexpected files remain
                drop(_inst_lock);
            }
        }
        pruned.push(id);
    }

    Ok(pruned)
}

#[allow(dead_code)]
pub fn prune_slice(
    cursor: &mut Option<fs::ReadDir>,
    slice_limit: usize,
    dry_run: bool,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<AgentInstanceId>> {
    let v1_dir = resolve_v1_dir()?;
    prune_slice_in_root(&v1_dir, cursor, slice_limit, dry_run, stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_atomic_update_leaves_last_whole_record_readable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        atomic_write_json(&path, &serde_json::json!({"revision":1})).unwrap();
        let previous = fs::read(&path).unwrap();
        assert!(atomic_write_json(&path, &"x".repeat(256 * 1024)).is_err());
        assert_eq!(fs::read(path).unwrap(), previous);
    }
    use std::sync::Mutex;
    use verij_types::agent::{compute_display_status, ActivityKind};
    use verij_types::identity::{HostKey, TurnId};
    use verij_types::identity::{SessionInstanceId, TerminalPaneId};

    static TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        orig_agent_state_dir: Option<std::ffi::OsString>,
        orig_xdg_state_home: Option<std::ffi::OsString>,
        _temp_dir: tempfile::TempDir,
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            match &self.orig_agent_state_dir {
                Some(val) => std::env::set_var("VERIJ_AGENT_STATE_DIR", val),
                None => std::env::remove_var("VERIJ_AGENT_STATE_DIR"),
            }
            match &self.orig_xdg_state_home {
                Some(val) => std::env::set_var("XDG_STATE_HOME", val),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

    fn temp_test_env() -> (TestEnvGuard, PathBuf, PathBuf) {
        let lock = TEST_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let orig_agent_state_dir = std::env::var_os("VERIJ_AGENT_STATE_DIR");
        let orig_xdg_state_home = std::env::var_os("XDG_STATE_HOME");

        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let state = dir.path().join("state");

        crate::process::ensure_private_directory(&runtime).unwrap();
        crate::process::ensure_private_directory(&state).unwrap();

        std::env::set_var("VERIJ_AGENT_STATE_DIR", &runtime);
        std::env::set_var("XDG_STATE_HOME", &state);

        let guard = TestEnvGuard {
            _lock: lock,
            orig_agent_state_dir,
            orig_xdg_state_home,
            _temp_dir: dir,
        };

        (guard, runtime, state)
    }

    #[test]
    fn test_register_and_idempotence() {
        let (_guard, _runtime, _state) = temp_test_env();

        let pane_key = PaneKey {
            session: SessionInstanceId("session-1".to_string()),
            terminal: TerminalPaneId(42),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 5678,
            boot_id: "boot-1".to_string(),
            uid: unsafe { libc::getuid() },
        };

        let id1 = register(pane_key.clone(), AgentKind::Agy, proc.clone(), true, None).unwrap();
        let id2 = register(pane_key.clone(), AgentKind::Agy, proc.clone(), true, None).unwrap();

        // Same verified process and pane must yield identical instance ID
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_registration_immutable_conflict_rejected() {
        let (_guard, _runtime, _state) = temp_test_env();

        let pane_key = PaneKey {
            session: SessionInstanceId("session-conflict".to_string()),
            terminal: TerminalPaneId(42),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 5678,
            boot_id: "boot-1".to_string(),
            uid: unsafe { libc::getuid() },
        };

        // First register as synthetic
        let _id1 = register(pane_key.clone(), AgentKind::Agy, proc.clone(), true, None).unwrap();

        // Attempting to register same pane and process as non-synthetic must be rejected with conflict
        let conflict = register(pane_key.clone(), AgentKind::Agy, proc.clone(), false, None);
        assert!(conflict.is_err());
    }

    #[test]
    fn test_registration_incomplete_missing_state_ignored() {
        let (_guard, runtime, _state) = temp_test_env();

        let v1_dir = runtime.join("agents").join("v1");
        crate::process::ensure_private_directory(&v1_dir).unwrap();

        // Create an incomplete directory with only identity.json
        let incomplete_dir = v1_dir.join("agent-incomplete");
        crate::process::ensure_private_directory(&incomplete_dir).unwrap();
        let incomplete_id = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId("agent-incomplete".to_string()),
            pane_key: PaneKey {
                session: SessionInstanceId("session-inc".to_string()),
                terminal: TerminalPaneId(1),
            },
            kind: AgentKind::Opencode,
            process: ProcessIdentity {
                pid: std::process::id(),
                start_jiffies: 10,
                boot_id: "boot".to_string(),
                uid: unsafe { libc::getuid() },
            },
            registered_at_ms: now_ms(),
            is_synthetic: true,
            runner_id: None,
        };
        atomic_write_json(&incomplete_dir.join("identity.json"), &incomplete_id).unwrap();

        // Inspect on incomplete registration fails
        assert!(inspect(&incomplete_id.agent_instance_id).is_err());
    }

    #[test]
    fn test_report_and_inspect() {
        let (_guard, _runtime, _state) = temp_test_env();

        let pane_key = PaneKey {
            session: SessionInstanceId("session-2".to_string()),
            terminal: TerminalPaneId(10),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 8765,
            boot_id: "boot-2".to_string(),
            uid: unsafe { libc::getuid() },
        };

        let inst = register(pane_key, AgentKind::Opencode, proc, true, None).unwrap();

        let partial = PartialSourceRecord {
            source_id: "opencode".to_string(),
            source_epoch: Some(1),
            source_revision: Some(1),
            activity: Some(ActivityKind::Working),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn-1".to_string())),
            ..Default::default()
        };

        let state = report(&inst, partial).unwrap();
        assert_eq!(state.reduced.status, AgentStatus::Working);

        let (inspect_id, inspect_state) = inspect(&inst).unwrap();
        assert_eq!(inspect_id.agent_instance_id, inst);
        assert_eq!(inspect_state.reduced.status, AgentStatus::Working);
    }

    #[test]
    fn test_acknowledgement_proof_validation_and_racing() {
        let (_guard, _runtime, _state) = temp_test_env();

        let pane_key = PaneKey {
            session: SessionInstanceId("session-ack".to_string()),
            terminal: TerminalPaneId(7),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 1111,
            boot_id: "boot-ack".to_string(),
            uid: unsafe { libc::getuid() },
        };

        let inst = register(pane_key.clone(), AgentKind::Agy, proc, true, None).unwrap();

        // Report turn 1 completion -> revision 1
        let stop_partial = PartialSourceRecord {
            source_id: "agy-lifecycle".to_string(),
            source_epoch: Some(1),
            source_revision: Some(1),
            activity: Some(ActivityKind::Idle),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn-1".to_string())),
            successful_completion: Some(verij_types::agent::SuccessfulCompletion {
                turn_id: Some(TurnId("turn-1".to_string())),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: now_ms(),
                summary: Some("success".to_string()),
            }),
            ..Default::default()
        };
        let state = report(&inst, stop_partial).unwrap();
        assert_eq!(state.reduced.status, AgentStatus::Done);
        assert_eq!(state.completion_revision, 1);

        let host = HostKey("host-alpha".to_string());

        // 1. Refuse proof if whole_host_focused is false
        let bad_proof = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 1,
            request_id: "req-1".to_string(),
            whole_host_focused: false,
            inner_focus_verified: true,
            verified_at_ms: now_ms(),
        };
        assert!(acknowledge(&bad_proof).is_err());

        // 2. Refuse proof if inner_focus_verified is false
        let bad_inner = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 1,
            request_id: "req-1".to_string(),
            whole_host_focused: true,
            inner_focus_verified: false,
            verified_at_ms: now_ms(),
        };
        assert!(acknowledge(&bad_inner).is_err());

        // 3. Refuse stale proof (> 2000 ms)
        let stale_proof = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 1,
            request_id: "req-1".to_string(),
            whole_host_focused: true,
            inner_focus_verified: true,
            verified_at_ms: now_ms().saturating_sub(3000),
        };
        assert!(acknowledge(&stale_proof).is_err());

        // 4. Valid proof acknowledges revision 1
        let valid_proof = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 1,
            request_id: "req-1".to_string(),
            whole_host_focused: true,
            inner_focus_verified: true,
            verified_at_ms: now_ms(),
        };
        let ack = acknowledge(&valid_proof).unwrap();
        assert_eq!(ack.acknowledged_revision, 1);
        let captured_identity = inspect(&inst).unwrap().0;
        let passive = VisitProof {
            host_key: HostKey("host-passive".into()),
            ..valid_proof.clone()
        };
        assert_eq!(
            acknowledge_current_if_current(&passive, &captured_identity, || true)
                .unwrap()
                .acknowledged_revision,
            1
        );

        // An obsolete visit waiting for the host lock must not publish an ack
        // after the new intent wins, even though its native proof was valid.
        let guarded_proof = VisitProof {
            host_key: HostKey("host-beta".into()),
            ..valid_proof.clone()
        };
        let hosts_dir = resolve_hosts_dir().unwrap();
        let lock = HostLock::acquire(&hosts_dir, "host-beta").unwrap();
        let current = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_current = current.clone();
        let (entered, waiting) = std::sync::mpsc::channel();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let worker = std::thread::spawn(move || {
            acknowledge_if_current(&guarded_proof, || {
                if calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 1 {
                    let _ = entered.send(());
                }
                worker_current.load(std::sync::atomic::Ordering::Acquire)
            })
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        current.store(false, std::sync::atomic::Ordering::Release);
        drop(lock);
        assert!(worker.join().unwrap().is_err());
        assert!(!hosts_dir.join("host-beta.json").exists());

        // For host-alpha, display status is now Idle
        assert_eq!(
            compute_display_status(&state.reduced, Some(&ack)),
            AgentStatus::Idle
        );

        // Another host (host-beta) has not acknowledged -> still Done
        let other_host_ack = None;
        assert_eq!(
            compute_display_status(&state.reduced, other_host_ack),
            AgentStatus::Done
        );

        // 5. Racing completion: agent completes turn 2 (revision 2)
        let turn2_partial = PartialSourceRecord {
            source_id: "agy-lifecycle".to_string(),
            source_epoch: Some(1),
            source_revision: Some(2),
            activity: Some(ActivityKind::Idle),
            turn_epoch: Some(1),
            turn_revision: Some(2),
            turn_id: Some(TurnId("turn-2".to_string())),
            successful_completion: Some(verij_types::agent::SuccessfulCompletion {
                turn_id: Some(TurnId("turn-2".to_string())),
                turn_epoch: 1,
                turn_revision: 2,
                completed_at_ms: now_ms(),
                summary: Some("turn 2 done".to_string()),
            }),
            ..Default::default()
        };
        let state2 = report(&inst, turn2_partial).unwrap();
        assert_eq!(state2.completion_revision, 2);
        let raced = VisitProof {
            host_key: HostKey("host-raced".into()),
            ..valid_proof.clone()
        };
        assert!(acknowledge_current_if_current(&raced, &captured_identity, || true).is_err());
        assert!(!resolve_hosts_dir()
            .unwrap()
            .join("host-raced.json")
            .exists());
        let current_proof = VisitProof {
            observed_completion_revision: 2,
            ..raced.clone()
        };
        let mut replaced_identity = captured_identity.clone();
        replaced_identity.process.start_jiffies += 1;
        assert!(
            acknowledge_current_if_current(&current_proof, &replaced_identity, || true).is_err()
        );

        // Stale navigation completed with observed_revision 1
        let stale_nav_proof = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 1,
            request_id: "req-stale".to_string(),
            whole_host_focused: true,
            inner_focus_verified: true,
            verified_at_ms: now_ms(),
        };
        let ack_stale = acknowledge(&stale_nav_proof).unwrap();
        // Ack stays 1, does NOT consume revision 2
        assert_eq!(ack_stale.acknowledged_revision, 1);

        // Display status for host-alpha is Done because revision 2 is unread!
        assert_eq!(
            compute_display_status(&state2.reduced, Some(&ack_stale)),
            AgentStatus::Done
        );

        // 6. Corrupt host store fails closed
        let hosts_dir = resolve_hosts_dir().unwrap();
        let host_file = hosts_dir.join(format!("{}.json", host.0));
        fs::write(&host_file, b"corrupted non-json content").unwrap();

        let future_proof = VisitProof {
            host_key: host.clone(),
            agent_instance_id: inst.clone(),
            pane_key: pane_key.clone(),
            observed_completion_revision: 2,
            request_id: "req-corr".to_string(),
            whole_host_focused: true,
            inner_focus_verified: true,
            verified_at_ms: now_ms(),
        };
        assert!(acknowledge(&future_proof).is_err());

        // A new turn can supersede Done without increasing the completed-turn
        // high-water mark. Passive eligibility must check reduced state too.
        let working = report(
            &inst,
            PartialSourceRecord {
                source_id: "agy-lifecycle".into(),
                source_epoch: Some(1),
                source_revision: Some(3),
                turn_epoch: Some(1),
                turn_revision: Some(3),
                turn_id: Some(TurnId("turn-3".into())),
                activity: Some(ActivityKind::Working),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(working.reduced.status, AgentStatus::Working);
        assert!(
            acknowledge_current_if_current(&current_proof, &captured_identity, || true).is_err()
        );
        assert!(!resolve_hosts_dir()
            .unwrap()
            .join("host-raced.json")
            .exists());
    }

    #[test]
    fn test_concurrent_registration_and_reports() {
        let (_guard, _runtime, _state) = temp_test_env();

        let pane_key = PaneKey {
            session: SessionInstanceId("session-concurrent".to_string()),
            terminal: TerminalPaneId(99),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 12345,
            boot_id: "boot-conc".to_string(),
            uid: unsafe { libc::getuid() },
        };

        // Spawn multiple concurrent registration attempts
        let mut handles = Vec::new();
        for _ in 0..8 {
            let pk = pane_key.clone();
            let p = proc.clone();
            handles.push(std::thread::spawn(move || {
                register(pk, AgentKind::Agy, p, true, None).unwrap()
            }));
        }

        let ids: Vec<AgentInstanceId> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // All threads must converge on the exact same instance ID
        let first_id = &ids[0];
        for id in &ids {
            assert_eq!(id, first_id);
        }

        // Spawn concurrent reports on this instance using accepted synthetic fixture sources
        let mut report_handles = Vec::new();
        for i in 1..=10 {
            let inst = first_id.clone();
            report_handles.push(std::thread::spawn(move || {
                let partial = PartialSourceRecord {
                    source_id: format!("fixture-worker-{}", i),
                    source_epoch: Some(1),
                    source_revision: Some(1),
                    activity: Some(ActivityKind::Working),
                    turn_epoch: Some(1),
                    turn_revision: Some(i),
                    turn_id: Some(TurnId(format!("turn-{}", i))),
                    ..Default::default()
                };
                report(&inst, partial).unwrap()
            }));
        }

        for h in report_handles {
            h.join().unwrap();
        }

        let (inspect_id, inspect_state) = inspect(first_id).unwrap();
        assert_eq!(&inspect_id.agent_instance_id, first_id);
        // All 10 worker sources were safely recorded under lock
        assert_eq!(inspect_state.sources.len(), 10);
    }

    #[test]
    fn test_prune_exited_process_and_tombstone() {
        let (_guard, _runtime, _state) = temp_test_env();

        // 1. Live process (this test process PID)
        let live_proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 10,
            boot_id: "boot-live".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let live_pane = PaneKey {
            session: SessionInstanceId("session-live".to_string()),
            terminal: TerminalPaneId(1),
        };
        let live_id = register(live_pane, AgentKind::Opencode, live_proc, true, None).unwrap();

        // 2. Dead process (PID 99999999 almost certainly nonexistent)
        let dead_proc = ProcessIdentity {
            pid: 99_999_999,
            start_jiffies: 10,
            boot_id: "boot-dead".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let dead_pane = PaneKey {
            session: SessionInstanceId("session-dead".to_string()),
            terminal: TerminalPaneId(2),
        };
        let dead_id = register(dead_pane, AgentKind::Opencode, dead_proc, true, None).unwrap();

        // Dry run: reports dead_id, doesn't delete
        let dry_pruned = prune(true).unwrap();
        assert_eq!(dry_pruned, vec![dead_id.clone()]);
        assert!(inspect(&dead_id).is_ok());

        // Real prune: deletes dead_id, keeps live_id
        let real_pruned = prune(false).unwrap();
        assert_eq!(real_pruned, vec![dead_id.clone()]);
        assert!(inspect(&dead_id).is_err());
        assert!(inspect(&live_id).is_ok());
    }

    #[test]
    fn test_read_bounded_bytes_rejects_oversized_and_fifo() {
        let dir = tempfile::tempdir().unwrap();

        // 1. Oversized file rejected before parsing
        let big_file = dir.path().join("big.json");
        fs::write(&big_file, vec![b'a'; 1000]).unwrap();
        let err = read_bounded_bytes(&big_file, 500).unwrap_err();
        assert!(err.to_string().contains("exceeds size limit"));

        // Normal file accepted
        let normal_file = dir.path().join("normal.json");
        fs::write(&normal_file, b"{\"hello\":\"world\"}").unwrap();
        let bytes = read_bounded_bytes(&normal_file, 500).unwrap();
        assert_eq!(bytes, b"{\"hello\":\"world\"}");

        // 2. FIFO (named pipe) rejected without blocking/hanging
        #[cfg(unix)]
        {
            let fifo_path = dir.path().join("test.fifo");
            let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
            let err = read_bounded_bytes(&fifo_path, 1024).unwrap_err();
            assert!(err.to_string().contains("not a regular file"));
        }
    }

    #[test]
    fn test_prune_slice_resumable_and_preserves_live() {
        let (_guard, _runtime, _state) = temp_test_env();
        let v1_dir = resolve_v1_dir().unwrap();

        // 1. Register live agent
        let live_proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 12345,
            boot_id: "boot-live".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let live_pane = PaneKey {
            session: SessionInstanceId("session-live".to_string()),
            terminal: TerminalPaneId(10),
        };
        let live_id = register(live_pane, AgentKind::Opencode, live_proc, true, None).unwrap();

        // 2. Register two dead agents
        let dead_proc1 = ProcessIdentity {
            pid: 99_999_998,
            start_jiffies: 11,
            boot_id: "boot-dead1".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let dead_id1 = register(
            PaneKey {
                session: SessionInstanceId("session-dead1".to_string()),
                terminal: TerminalPaneId(11),
            },
            AgentKind::Opencode,
            dead_proc1,
            true,
            None,
        )
        .unwrap();

        let dead_proc2 = ProcessIdentity {
            pid: 99_999_999,
            start_jiffies: 12,
            boot_id: "boot-dead2".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let dead_id2 = register(
            PaneKey {
                session: SessionInstanceId("session-dead2".to_string()),
                terminal: TerminalPaneId(12),
            },
            AgentKind::Opencode,
            dead_proc2,
            true,
            None,
        )
        .unwrap();

        // Use prune_slice_in_root with slice limit = 1 and persistent cursor
        let mut cursor = None;
        let mut total_pruned = Vec::new();
        for _ in 0..10 {
            let chunk = prune_slice_in_root(&v1_dir, &mut cursor, 1, false, None).unwrap();
            total_pruned.extend(chunk);
            if cursor.is_none() {
                break;
            }
        }

        assert!(total_pruned.contains(&dead_id1));
        assert!(total_pruned.contains(&dead_id2));
        assert!(!total_pruned.contains(&live_id));
        assert!(inspect(&live_id).is_ok());
        assert!(inspect(&dead_id1).is_err());
        assert!(inspect(&dead_id2).is_err());
    }

    #[test]
    fn test_prune_skips_contended_instance_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent_dir = root.join("agent-contended");
        fs::create_dir_all(&agent_dir).unwrap();

        let dead_proc = ProcessIdentity {
            pid: 99_999_998,
            start_jiffies: 11,
            boot_id: "boot-dead".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let identity = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId("agent-contended".to_string()),
            pane_key: PaneKey {
                session: SessionInstanceId("s1".into()),
                terminal: TerminalPaneId(1),
            },
            kind: AgentKind::Opencode,
            process: dead_proc,
            registered_at_ms: 1000,
            is_synthetic: true,
            runner_id: None,
        };
        atomic_write_json(&agent_dir.join("identity.json"), &identity).unwrap();
        atomic_write_json(
            &agent_dir.join("state.json"),
            &serde_json::json!({"revision": 1}),
        )
        .unwrap();

        // Hold exclusive lock on instance
        let lock = InstanceLock::acquire(&agent_dir).unwrap();

        let mut cursor = None;
        let pruned = prune_slice_in_root(root, &mut cursor, 10, false, None).unwrap();
        assert!(
            pruned.is_empty(),
            "Contended instance lock must be skipped without blocking or error"
        );
        assert!(agent_dir.join("identity.json").exists());

        // Release lock and prune again
        drop(lock);
        let mut cursor = None;
        let pruned = prune_slice_in_root(root, &mut cursor, 10, false, None).unwrap();
        assert_eq!(pruned, vec![AgentInstanceId("agent-contended".to_string())]);
        assert!(!agent_dir.exists());
    }

    #[test]
    fn test_prune_preserves_schema_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent_dir = root.join("agent-schema-mismatch");
        fs::create_dir_all(&agent_dir).unwrap();

        let dead_proc = ProcessIdentity {
            pid: 99_999_998,
            start_jiffies: 11,
            boot_id: "boot-dead".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let identity = AgentIdentity {
            schema_version: 9999, // unsupported/mismatched schema version
            agent_instance_id: AgentInstanceId("agent-schema-mismatch".to_string()),
            pane_key: PaneKey {
                session: SessionInstanceId("s1".into()),
                terminal: TerminalPaneId(1),
            },
            kind: AgentKind::Opencode,
            process: dead_proc,
            registered_at_ms: 1000,
            is_synthetic: true,
            runner_id: None,
        };
        atomic_write_json(&agent_dir.join("identity.json"), &identity).unwrap();

        let mut cursor = None;
        let pruned = prune_slice_in_root(root, &mut cursor, 10, false, None).unwrap();
        assert!(pruned.is_empty());
        assert!(agent_dir.join("identity.json").exists());
    }

    #[test]
    fn test_prune_safe_removal_preserves_lock_and_tombstone_on_unexpected_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent_dir = root.join("agent-unexpected-files");
        fs::create_dir_all(&agent_dir).unwrap();

        let dead_proc = ProcessIdentity {
            pid: 99_999_998,
            start_jiffies: 11,
            boot_id: "boot-dead".to_string(),
            uid: unsafe { libc::getuid() },
        };
        let identity = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId("agent-unexpected-files".to_string()),
            pane_key: PaneKey {
                session: SessionInstanceId("s1".into()),
                terminal: TerminalPaneId(1),
            },
            kind: AgentKind::Opencode,
            process: dead_proc,
            registered_at_ms: 1000,
            is_synthetic: true,
            runner_id: None,
        };
        atomic_write_json(&agent_dir.join("identity.json"), &identity).unwrap();
        atomic_write_json(
            &agent_dir.join("state.json"),
            &serde_json::json!({"revision": 1}),
        )
        .unwrap();

        // Write an unexpected file into agent_dir
        fs::write(agent_dir.join("unexpected_log.txt"), b"log data").unwrap();

        let mut cursor = None;
        let pruned = prune_slice_in_root(root, &mut cursor, 10, false, None).unwrap();
        assert_eq!(
            pruned,
            vec![AgentInstanceId("agent-unexpected-files".to_string())]
        );

        // State files removed
        assert!(!agent_dir.join("identity.json").exists());
        assert!(!agent_dir.join("state.json").exists());

        // Unexpected file, .tombstone, and .lock preserved (to avoid lock splitting)
        assert!(agent_dir.join("unexpected_log.txt").exists());
        assert!(agent_dir.join(".tombstone").exists());
        assert!(agent_dir.join(".lock").exists());
        assert!(agent_dir.exists());
    }

    #[test]
    fn test_registration_cursor_validations() {
        let (_guard, runtime, _state) = temp_test_env();
        let v1_dir = runtime.join("agents").join("v1");
        crate::process::ensure_private_directory(&v1_dir).unwrap();

        let pane_key = PaneKey {
            session: SessionInstanceId("s-cursor".to_string()),
            terminal: TerminalPaneId(5),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 555,
            boot_id: "boot-c".to_string(),
            uid: unsafe { libc::getuid() },
        };

        // 1. Existing dir with .tombstone is skipped
        let tomb_dir = v1_dir.join("agent-tomb");
        crate::process::ensure_private_directory(&tomb_dir).unwrap();
        let tomb_ident = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId("agent-tomb".to_string()),
            pane_key: pane_key.clone(),
            kind: AgentKind::Agy,
            process: proc.clone(),
            registered_at_ms: 100,
            is_synthetic: true,
            runner_id: None,
        };
        atomic_write_json(&tomb_dir.join("identity.json"), &tomb_ident).unwrap();
        atomic_write_json(&tomb_dir.join("state.json"), &serde_json::json!({"rev": 1})).unwrap();
        fs::write(tomb_dir.join(".tombstone"), b"").unwrap();

        // 2. Existing dir with mismatched schema_version is skipped
        let schema_dir = v1_dir.join("agent-schema");
        crate::process::ensure_private_directory(&schema_dir).unwrap();
        let mut schema_ident = tomb_ident.clone();
        schema_ident.agent_instance_id = AgentInstanceId("agent-schema".to_string());
        schema_ident.schema_version = 9999;
        atomic_write_json(&schema_dir.join("identity.json"), &schema_ident).unwrap();
        atomic_write_json(
            &schema_dir.join("state.json"),
            &serde_json::json!({"rev": 1}),
        )
        .unwrap();

        // 3. Existing dir where directory name != agent_instance_id.0 is skipped
        let mismatch_dir = v1_dir.join("agent-dir-mismatch");
        crate::process::ensure_private_directory(&mismatch_dir).unwrap();
        let mut mismatch_ident = tomb_ident.clone();
        mismatch_ident.agent_instance_id = AgentInstanceId("agent-different-id".to_string());
        atomic_write_json(&mismatch_dir.join("identity.json"), &mismatch_ident).unwrap();
        atomic_write_json(
            &mismatch_dir.join("state.json"),
            &serde_json::json!({"rev": 1}),
        )
        .unwrap();

        // Stepping cursor should skip all 3 invalid records and allocate a new one at EOF
        let mut cursor =
            RegistrationCursor::new(pane_key.clone(), AgentKind::Agy, proc.clone(), true, None)
                .unwrap();
        let outcome = cursor.step(None).unwrap();
        let allocated_id = match outcome {
            RegistrationOutcome::Complete(id) => id,
            RegistrationOutcome::Pending => panic!("Expected allocation on EOF"),
        };
        assert_ne!(allocated_id.0, "agent-tomb");
        assert_ne!(allocated_id.0, "agent-schema");
        assert_ne!(allocated_id.0, "agent-dir-mismatch");
        assert_ne!(allocated_id.0, "agent-different-id");

        // Now run a second cursor for the same pane and process: should find the newly allocated record
        let mut cursor2 =
            RegistrationCursor::new(pane_key, AgentKind::Agy, proc, true, None).unwrap();
        let outcome2 = cursor2.step(None).unwrap();
        match outcome2 {
            RegistrationOutcome::Complete(id) => assert_eq!(id, allocated_id),
            RegistrationOutcome::Pending => panic!("Expected finding existing valid ID"),
        }
    }
}
