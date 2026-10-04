//! Linux process birth and actual pane/foreground terminal ownership.
//! An inherited pane environment or process name is never sufficient.
use anyhow::{bail, Context, Result};
use std::path::Path;
use verij_types::identity::ProcessIdentity;

#[derive(Debug, Clone)]
struct Stat {
    state: String,
    parent: u32,
    group: i64,
    session: i64,
    tty: i64,
    foreground: i64,
    start: u64,
}

fn parse_stat(text: &str) -> Result<Stat> {
    // comm may itself contain spaces and parentheses; fields follow its final ')'.
    let (_, suffix) = text.rsplit_once(')').context("invalid process stat")?;
    let fields: Vec<_> = suffix.split_whitespace().collect();
    let field = |index: usize| {
        fields
            .get(index)
            .copied()
            .context("incomplete process stat")
    };
    Ok(Stat {
        state: field(0)?.into(),
        parent: field(1)?.parse()?,
        group: field(2)?.parse()?,
        session: field(3)?.parse()?,
        tty: field(4)?.parse()?,
        foreground: field(5)?.parse()?,
        start: field(19)?.parse()?,
    })
}

fn stat(pid: u32) -> Result<Stat> {
    let result = parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat"))?)?;
    if matches!(result.state.as_str(), "Z" | "X" | "x") {
        bail!("process {pid} has exited");
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
pub fn identity(pid: u32) -> Result<ProcessIdentity> {
    use std::os::unix::fs::MetadataExt;
    if pid == 0 {
        bail!("invalid process PID");
    }
    let before = stat(pid)?;
    let uid = std::fs::metadata(format!("/proc/{pid}"))?.uid();
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_string();
    let after = stat(pid)?;
    if before.start != after.start {
        bail!("process changed during identity query");
    }
    Ok(ProcessIdentity {
        pid,
        start_jiffies: after.start,
        boot_id,
        uid,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn identity(_: u32) -> Result<ProcessIdentity> {
    bail!("verified process binding is unavailable on this platform")
}

pub fn is_alive(expected: &ProcessIdentity) -> bool {
    identity(expected.pid)
        .as_ref()
        .is_ok_and(|actual| actual == expected)
}

/// Wall-clock lower bound for a Linux process birth, used only to reject stale
/// exporter files from an earlier process that reused the same numeric PID.
/// It is not an event ordering clock.
pub fn started_at_ms(expected: &ProcessIdentity) -> Result<u64> {
    let boot_seconds: u64 = std::fs::read_to_string("/proc/stat")?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .context("missing boot time")?
        .parse()?;
    #[cfg(unix)]
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    #[cfg(not(unix))]
    let ticks = 0;
    if ticks <= 0 {
        bail!("process clock tick rate unavailable");
    }
    Ok(boot_seconds * 1000 + expected.start_jiffies * 1000 / ticks as u64)
}

#[cfg(target_os = "linux")]
fn tty_descriptor(pid: u32, descriptor: u8) -> Result<u64> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let info = std::fs::metadata(format!("/proc/{pid}/fd/{descriptor}"))?;
    if !info.file_type().is_char_device() {
        bail!("process stdio is not a terminal");
    }
    // Character devices other than the process's actual controlling tty are
    // excluded by the matching pane + tty_nr checks below.
    Ok(info.rdev())
}

fn descendant(pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..128 {
        if current == ancestor {
            return true;
        }
        let Ok(value) = stat(current) else {
            return false;
        };
        if value.parent == 0 || value.parent == current {
            return false;
        }
        current = value.parent;
    }
    false
}

/// Only a live, same-user process rendering through this pane's foreground tty
/// can register an interactive agent. Redirected/detached children are excluded.
#[cfg(target_os = "linux")]
pub fn verify_foreground(
    server: &ProcessIdentity,
    pane: &ProcessIdentity,
    agent: &ProcessIdentity,
) -> Result<()> {
    let uid = std::fs::metadata("/proc/self")?;
    use std::os::unix::fs::MetadataExt;
    if [server, pane, agent]
        .iter()
        .any(|process| process.uid != uid.uid() || !is_alive(process))
    {
        bail!("pane ownership processes are not live and same-user");
    }
    if !descendant(pane.pid, server.pid) || !descendant(agent.pid, pane.pid) {
        bail!("agent does not descend from this server's terminal pane");
    }
    let pane_stat = stat(pane.pid)?;
    let agent_stat = stat(agent.pid)?;
    if agent_stat.tty == 0
        || agent_stat.tty != pane_stat.tty
        || agent_stat.session != pane_stat.session
        || agent_stat.group != agent_stat.foreground
        || agent_stat.foreground <= 0
    {
        bail!("agent does not own this pane's foreground terminal job");
    }
    let pane_tty = tty_descriptor(pane.pid, 0)?;
    if tty_descriptor(agent.pid, 0)? != pane_tty || tty_descriptor(agent.pid, 1)? != pane_tty {
        bail!("agent input/output is redirected away from this pane");
    }
    if [server, pane, agent]
        .iter()
        .any(|process| !is_alive(process))
    {
        bail!("pane ownership changed during validation");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn verify_foreground(
    _: &ProcessIdentity,
    _: &ProcessIdentity,
    _: &ProcessIdentity,
) -> Result<()> {
    bail!("foreground ownership is unavailable on this platform")
}

pub fn ensure_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(path)?;
        let own = std::fs::metadata("/proc/self")?;
        if !metadata.is_dir() || metadata.uid() != own.uid() {
            bail!("runtime directory is not owned by this user");
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_comm_does_not_shift_birth_or_terminal_fields() {
        let text = format!("42 (odd ) name) S 5 42 42 34816 42 {}987", "0 ".repeat(13));
        let value = parse_stat(&text).unwrap();
        assert_eq!(
            (
                value.parent,
                value.group,
                value.session,
                value.tty,
                value.foreground,
                value.start
            ),
            (5, 42, 42, 34816, 42, 987)
        );
        assert!(parse_stat("42 (truncated) S 5").is_err());
    }
    #[test]
    fn reused_pid_or_boot_never_matches_live_process() {
        let owner = identity(std::process::id()).unwrap();
        assert!(is_alive(&owner));
        let mut stale = owner.clone();
        stale.start_jiffies += 1;
        assert!(!is_alive(&stale));
        stale = owner;
        stale.boot_id.push('x');
        assert!(!is_alive(&stale));
    }
}
