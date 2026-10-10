//! CLI command surface for agent monitoring.
//!
//! Exposes `AgentCommand` dispatched by `verij agent ...`.
//! Handles registration against Zellij inventory, bounded JSON reporting,
//! verified acknowledgement with visit proofs, and serialized pruning.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::io::{self, Read};

use crate::agent_store;
use verij_types::agent::{AgentInstanceId, AgentKind, PartialSourceRecord};
use verij_types::identity::{HostKey, PaneKey, SessionInstanceId, TerminalPaneId};

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Install a pane-local adapter (restart the agent afterward).
    Setup(crate::agent_setup::SetupArgs),
    /// Remove only Verij-owned integration entries and assets.
    Uninstall(crate::agent_setup::SetupArgs),
    /// Inspect adapter installation, supported versions and runtime health.
    Doctor(crate::agent_setup::DoctorArgs),
    /// Internal bounded, ordered OpenCode TUI reporter stream.
    #[command(hide = true)]
    Opencode {
        #[arg(long)] session: String,
        #[arg(long)] pane: u32,
        #[arg(long)] pid: u32,
    },
    /// Internal metadata-only Agy callback reporter.
    #[command(hide = true)]
    Agy {
        #[arg(long)] pane: u32,
        #[arg(long)] pid: u32,
        #[arg(long)] birth: u64,
    },
    /// Observe pane-backed Magy watches (also runs with each sidebar).
    Magy {
        /// Reconcile once, rather than observing until interrupted.
        #[arg(long)] once: bool,
    },
    /// Validate actual foreground pane/process ownership without registration.
    Verify {
        #[arg(long)]
        session: String,
        #[arg(long)]
        pane: u32,
        #[arg(long)]
        pid: u32,
    },
    /// Register a monitored agent instance in a terminal pane.
    Register(RegisterArgs),

    /// Report source observations via bounded JSON stdin.
    Report(ReportArgs),

    /// Inspect agent identity and reduced execution state.
    Inspect(InspectArgs),

    /// Acknowledge completion visit with fresh verified proof.
    Ack(AckArgs),

    /// Prune dead agent records whose processes have exited.
    Prune(PruneArgs),

    /// Create a development fixture marked synthetic for H1/H2 testing.
    Fixture(FixtureArgs),
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    /// Zellij session name.
    #[arg(long)]
    pub session: String,

    /// Terminal pane numeric ID.
    #[arg(long)]
    pub pane: u32,

    /// Monitored agent process ID.
    #[arg(long)]
    pub pid: u32,

    /// Agent kind ("opencode" or "agy").
    #[arg(long)]
    pub kind: String,

    /// Explicitly mark registration synthetic (development fixture only).
    #[arg(long)]
    pub synthetic: bool,

    /// Optional runner identity (e.g. Magy watch_id).
    #[arg(long)]
    pub runner_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Monitored agent instance ID.
    #[arg(long)]
    pub instance: String,
}

#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Monitored agent instance ID.
    #[arg(long)]
    pub instance: String,
}

#[derive(Debug, Args)]
pub struct AckArgs {
    /// Monitored agent instance ID.
    #[arg(long)]
    pub instance: String,

    /// Host key acknowledging visit.
    #[arg(long)]
    pub host: String,

    /// Monotonic completion revision observed at start of navigation.
    #[arg(long)]
    pub revision: u64,
}

#[derive(Debug, Args)]
pub struct PruneArgs {
    /// List records that would be removed without deleting.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct FixtureArgs {
    /// Session name for synthetic fixture.
    #[arg(long, default_value = "fixture-session")]
    pub session: String,

    /// Terminal pane ID.
    #[arg(long, default_value_t = 1)]
    pub pane: u32,

    /// Agent kind ("opencode" or "agy").
    #[arg(long, default_value = "opencode")]
    pub kind: String,
}

/// Execute agent CLI subcommands.
pub fn execute(command: AgentCommand) -> Result<()> {
    match command {
        AgentCommand::Setup(args) => crate::agent_setup::setup(args),
        AgentCommand::Uninstall(args) => crate::agent_setup::uninstall(args),
        AgentCommand::Doctor(args) => crate::agent_setup::doctor(args),
        AgentCommand::Opencode { session, pane, pid } => crate::opencode::run(session, pane, pid),
        AgentCommand::Agy { pane, pid, birth } => crate::agy::run(pane, pid, birth),
        AgentCommand::Magy { once } => crate::magy::run(once),
        AgentCommand::Verify { session, pane, pid } => {
            let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
            let inventory = snapshots
                .iter()
                .find(|snapshot| snapshot.name == session)
                .and_then(|snapshot| snapshot.inventory.as_ref())
                .context("session has no verified pane inventory")?;
            let server = inventory
                .server_process
                .as_ref()
                .context("server incarnation unavailable")?;
            let pane = inventory
                .panes
                .iter()
                .find(|candidate| candidate.terminal_id.0 == pane && !candidate.exited)
                .and_then(|pane| pane.pane_process.as_ref())
                .context("pane process unavailable")?;
            let agent = crate::process::identity(pid)?;
            crate::process::verify_foreground(server, pane, &agent)?;
            println!(
                "{}",
                serde_json::json!({"foreground_binding_verified":true,"process":agent})
            );
            Ok(())
        }
        AgentCommand::Register(args) => execute_register(args),
        AgentCommand::Report(args) => execute_report(args),
        AgentCommand::Inspect(args) => execute_inspect(args),
        AgentCommand::Ack(args) => execute_ack(args),
        AgentCommand::Prune(args) => execute_prune(args),
        AgentCommand::Fixture(args) => execute_fixture(args),
    }
}

fn execute_register(args: RegisterArgs) -> Result<()> {
    let synthetic = args.synthetic;
    let instance_id = register(args)?;
    println!("{}", serde_json::json!({
        "status": "registered", "agent_instance_id": instance_id.0, "is_synthetic": synthetic
    }));
    Ok(())
}

pub fn register(args: RegisterArgs) -> Result<AgentInstanceId> {
    if args.runner_id.is_some() && !args.synthetic {
        bail!("runner binding is owned by agent magy; generic registration cannot supply it");
    }
    let kind = match args.kind.to_lowercase().as_str() {
        "opencode" => AgentKind::Opencode,
        "agy" => AgentKind::Agy,
        other => bail!(
            "unsupported agent kind {:?}, must be 'opencode' or 'agy'",
            other
        ),
    };

    let states_dir = crate::fs_watcher::resolve_states_dir();
    let snapshots = crate::inventory::read_states(&states_dir);

    let session_snapshot = snapshots.iter().find(|s| s.name == args.session);

    let (pane_key, process) = if args.synthetic {
        let session_instance_id = session_snapshot
            .and_then(|s| s.inventory.as_ref())
            .and_then(|inv| inv.session_instance_id.clone())
            .unwrap_or_else(|| SessionInstanceId(format!("synthetic:{}", args.session)));
        let pane_key = PaneKey {
            session: session_instance_id,
            terminal: TerminalPaneId(args.pane),
        };
        let proc = crate::process::identity(args.pid).unwrap_or_else(|_| {
            verij_types::identity::ProcessIdentity {
                pid: args.pid,
                start_jiffies: 0,
                boot_id: "synthetic".to_string(),
                uid: unsafe { libc::getuid() },
            }
        });
        (pane_key, proc)
    } else {
        let snapshot = session_snapshot.ok_or_else(|| {
            anyhow::anyhow!(
                "session {:?} not found in inventory at {:?}",
                args.session,
                states_dir
            )
        })?;

        let inventory = snapshot.inventory.as_ref().ok_or_else(|| {
            anyhow::anyhow!("session {:?} has no inventory metadata", args.session)
        })?;

        let server_process = inventory.server_process.as_ref().ok_or_else(|| {
            anyhow::anyhow!("session {:?} has no verified server_process", args.session)
        })?;

        let session_instance_id = inventory
            .session_instance_id
            .clone()
            .unwrap_or_else(|| SessionInstanceId::from_process(server_process));

        let pane_snapshot = inventory
            .panes
            .iter()
            .find(|p| p.terminal_id.0 == args.pane)
            .ok_or_else(|| {
                anyhow::anyhow!("pane {} not found in session {:?}", args.pane, args.session)
            })?;

        let pane_process = pane_snapshot
            .pane_process
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pane {} has no verified pane_process", args.pane))?;

        let agent_process = crate::process::identity(args.pid)
            .with_context(|| format!("failed to read identity for pid {}", args.pid))?;

        (if kind == AgentKind::Agy {
            crate::process::verify_agy_foreground(server_process, pane_process, &agent_process)
        } else {
            crate::process::verify_foreground(server_process, pane_process, &agent_process)
        })
            .with_context(|| {
                format!(
                    "foreground verification failed for pid {} in pane {}",
                    args.pid, args.pane
                )
            })?;

        let pane_key = PaneKey {
            session: session_instance_id,
            terminal: TerminalPaneId(args.pane),
        };
        (pane_key, agent_process)
    };

    agent_store::register(pane_key, kind, process, args.synthetic, args.runner_id)
}

fn execute_report(args: ReportArgs) -> Result<()> {
    let mut stdin_buf = Vec::new();
    // Bounded read (max 256 KB)
    io::stdin()
        .take(262_145)
        .read_to_end(&mut stdin_buf)
        .context("failed to read from stdin")?;

    if stdin_buf.is_empty() {
        bail!("empty report payload on stdin");
    }
    if stdin_buf.len() > 262_144 {
        bail!("report payload exceeds 256 KiB limit");
    }

    let partial: PartialSourceRecord = serde_json::from_slice(&stdin_buf)
        .context("failed to parse PartialSourceRecord JSON from stdin")?;

    let instance_id = AgentInstanceId(args.instance);
    let state = agent_store::report(&instance_id, partial)?;

    println!(
        "{}",
        serde_json::json!({
            "status": "ok",
            "record_revision": state.record_revision,
            "completion_revision": state.completion_revision,
            "reduced_status": state.reduced.status
        })
    );

    Ok(())
}

fn execute_inspect(args: InspectArgs) -> Result<()> {
    let instance_id = AgentInstanceId(args.instance);
    let (identity, state) = agent_store::inspect(&instance_id)?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "identity": identity,
            "state": state
        }))?
    );

    Ok(())
}

fn execute_ack(args: AckArgs) -> Result<()> {
    let ack = acknowledge_visit(args)?;
    println!("{}", serde_json::to_string(&ack)?);
    Ok(())
}

/// UI callers pass only the revision they actually observed. Native focus and
/// ownership produce the proof; the selected sidebar row is not evidence.
pub fn acknowledge_visit(args: AckArgs) -> Result<verij_types::agent::InstanceAck> {
    acknowledge_visit_if_current(args, || true)
}

pub fn acknowledge_visit_if_current(
    args: AckArgs,
    is_current: impl Fn() -> bool,
) -> Result<verij_types::agent::InstanceAck> {
    if !is_current() {
        bail!("visit superseded locally");
    }
    let instance = AgentInstanceId(args.instance);
    let (identity, state) = agent_store::inspect(&instance)?;
    if args.revision == 0 || args.revision > state.completion_revision {
        bail!("completion revision was not observed");
    }
    let root = crate::navigation::resolve_control_dir();
    let (result, verified_at_ms) = crate::navigation::verified_visit(&root, &args.host)
        .context("fresh whole-host visit unavailable")?;
    let observation = result
        .observation
        .context("inner focus observation missing")?;
    let binding = crate::navigation::load_binding(&root, &args.host)?;
    if observation.terminal != Some(identity.pane_key.terminal)
        || SessionInstanceId::from_process(&binding.server_process) != identity.pane_key.session
    {
        bail!("confirmed visit targets another pane or server lifetime");
    }
    let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
    let inventory = snapshots
        .iter()
        .find(|snapshot| Some(&snapshot.name) == observation.session_name.as_ref())
        .and_then(|snapshot| snapshot.inventory.as_ref())
        .context("visited pane topology unavailable")?;
    let server = inventory
        .server_process
        .as_ref()
        .context("visited server incarnation unavailable")?;
    if *server != binding.server_process {
        bail!("visited server identity changed");
    }
    let pane = inventory
        .panes
        .iter()
        .find(|pane| pane.terminal_id == identity.pane_key.terminal)
        .and_then(|pane| pane.pane_process.as_ref())
        .context("visited terminal process unavailable")?;
    if !identity.is_synthetic {
        crate::magy::verify_owner(&identity, server, pane)?;
    }
    let proof = verij_types::agent::VisitProof {
        host_key: HostKey(args.host),
        agent_instance_id: instance,
        pane_key: identity.pane_key,
        observed_completion_revision: args.revision,
        request_id: result.request.request_id,
        whole_host_focused: result.whole_host_verified,
        inner_focus_verified: true,
        verified_at_ms,
    };
    if !is_current() {
        bail!("visit superseded locally before acknowledgement");
    }
    let ack = agent_store::acknowledge_if_current(&proof, is_current)?;
    Ok(ack)
}

fn execute_prune(args: PruneArgs) -> Result<()> {
    let pruned = agent_store::prune(args.dry_run)?;

    let pruned_ids: Vec<String> = pruned.into_iter().map(|id| id.0).collect();
    println!(
        "{}",
        serde_json::json!({
            "dry_run": args.dry_run,
            "pruned": pruned_ids
        })
    );

    Ok(())
}

fn execute_fixture(args: FixtureArgs) -> Result<()> {
    let kind = match args.kind.to_lowercase().as_str() {
        "opencode" => AgentKind::Opencode,
        "agy" => AgentKind::Agy,
        _ => AgentKind::Opencode,
    };
    let pane_key = PaneKey {
        session: SessionInstanceId(format!("synthetic:{}", args.session)),
        terminal: TerminalPaneId(args.pane),
    };
    let proc = verij_types::identity::ProcessIdentity {
        pid: std::process::id(),
        start_jiffies: 100,
        boot_id: "fixture-boot".to_string(),
        uid: unsafe { libc::getuid() },
    };

    let instance_id = agent_store::register(pane_key, kind, proc, true, None)?;
    println!(
        "{}",
        serde_json::json!({
            "status": "created_fixture",
            "agent_instance_id": instance_id.0,
            "is_synthetic": true
        })
    );

    Ok(())
}
