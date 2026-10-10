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

/// Observe Linux AF_UNIX socket peers without reading Zellij's private IPC.
/// This qualifies the server hint before a registration nonce is written: an
/// old marker after native session switching must not type into another agent.
#[cfg(target_os = "linux")]
pub fn verify_server_connection(client: &ProcessIdentity, server: &ProcessIdentity) -> Result<()> {
    use std::collections::BTreeSet;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    fn sockets(pid: u32) -> Result<BTreeSet<u32>> {
        Ok(std::fs::read_dir(format!("/proc/{pid}/fd"))?.flatten().filter_map(|entry| {
            let link = std::fs::read_link(entry.path()).ok()?;
            link.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
        }).collect())
    }
    if !is_alive(client) || !is_alive(server) { bail!("registration connection owner exited"); }
    let client_sockets = sockets(client.pid)?;
    let server_sockets = sockets(server.pid)?;
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 4) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    let connected = unsafe { libc::connect(fd.as_raw_fd(), (&address as *const libc::sockaddr_nl).cast(), std::mem::size_of_val(&address) as libc::socklen_t) };
    if connected < 0 { return Err(std::io::Error::last_os_error().into()); }
    let timeout = libc::timeval { tv_sec: 1, tv_usec: 0 };
    if unsafe { libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO,
        (&timeout as *const libc::timeval).cast(), std::mem::size_of_val(&timeout) as libc::socklen_t) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // Linux UAPI nlmsghdr + unix_diag_req: SOCK_DIAG_BY_FAMILY,
    // NLM_F_REQUEST|NLM_F_DUMP, UDIAG_SHOW_PEER. No message/prompt payloads.
    let mut request = Vec::new();
    request.extend(40u32.to_ne_bytes()); request.extend(20u16.to_ne_bytes());
    request.extend(0x301u16.to_ne_bytes()); request.extend(1u32.to_ne_bytes()); request.extend(0u32.to_ne_bytes());
    request.extend([libc::AF_UNIX as u8, 0, 0, 0]); request.extend(u32::MAX.to_ne_bytes());
    request.extend(0u32.to_ne_bytes()); request.extend(4u32.to_ne_bytes());
    request.extend(u32::MAX.to_ne_bytes()); request.extend(u32::MAX.to_ne_bytes());
    if unsafe { libc::send(fd.as_raw_fd(), request.as_ptr().cast(), request.len(), 0) } != request.len() as isize {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut buffer = vec![0u8; 65536];
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(2) {
        let size = unsafe { libc::recv(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if size <= 0 { return Err(std::io::Error::last_os_error().into()); }
        let bytes = &buffer[..size as usize];
        let mut offset = 0;
        while offset + 16 <= bytes.len() {
            let length = u32::from_ne_bytes(bytes[offset..offset + 4].try_into()?) as usize;
            if length < 16 || offset + length > bytes.len() { bail!("incomplete socket diagnostic"); }
            let kind = u16::from_ne_bytes(bytes[offset + 4..offset + 6].try_into()?);
            if kind == 3 { bail!("Workspace client is not connected to the hinted inner server; attachment hint is stale"); }
            if kind == 2 { bail!("OS socket-peer diagnostic unavailable"); }
            if length >= 32 && bytes[offset + 16] == libc::AF_UNIX as u8 {
                let inode = u32::from_ne_bytes(bytes[offset + 20..offset + 24].try_into()?);
                let mut attribute = offset + 32;
                while attribute + 4 <= offset + length {
                    let len = u16::from_ne_bytes(bytes[attribute..attribute + 2].try_into()?) as usize;
                    let kind = u16::from_ne_bytes(bytes[attribute + 2..attribute + 4].try_into()?) & 0x3fff;
                    if len < 4 || attribute + len > offset + length { bail!("invalid socket-peer attribute"); }
                    if kind == 2 && len >= 8 {
                        let peer = u32::from_ne_bytes(bytes[attribute + 4..attribute + 8].try_into()?);
                        if client_sockets.contains(&inode) && server_sockets.contains(&peer)
                            && sockets(client.pid)?.contains(&inode) && sockets(server.pid)?.contains(&peer)
                            && is_alive(client) && is_alive(server) { return Ok(()); }
                    }
                    attribute += (len + 3) & !3;
                }
            }
            offset += (length + 3) & !3;
        }
    }
    bail!("OS socket-peer diagnostic timed out")
}

#[cfg(not(target_os = "linux"))]
pub fn verify_server_connection(_: &ProcessIdentity, _: &ProcessIdentity) -> Result<()> {
    bail!("verified server connection is unavailable on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "linux")]
    fn socket_peer_connection_is_observed_from_kernel() {
        let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        let process = identity(std::process::id()).unwrap();
        verify_server_connection(&process, &process).unwrap();
        drop((left, right));
        let mut stale = process.clone(); stale.start_jiffies += 1;
        assert!(verify_server_connection(&stale, &process).is_err());
    }
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
