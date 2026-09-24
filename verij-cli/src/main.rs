/// verij-cli/src/main.rs
///
/// Entry point for the `verij` binary.
///
/// Dispatches CLI subcommands via clap:
///   - `verij start`  — launch a new Verij Host Session with dynamic layout.
///   - `verij attach` — attach to an existing Verij Host Session.
///   - `verij ui`     — launch the Ratatui TUI sidebar (used inside host layout).
use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

mod actions;
mod config;
mod fs_watcher;
mod layout;
mod registry;
mod session;
mod tui;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "verij",
    about = "Workspace and session manager for Zellij",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Verij Host Session.
    ///
    /// Renders user-owned Verij layout, then starts or attaches host session.
    Start(StartArgs),

    /// Attach to an existing Verij Host Session.
    Attach(AttachArgs),

    /// Launch the interactive TUI sidebar directly.
    ///
    /// Connects to the verij-plugin via a Zellij native pipe and renders a
    /// live, navigable 2-level tree of sessions and tabs.
    Ui,

    /// Configuration helpers.
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Create missing config.toml and verij.kdl without overwriting user edits.
    Init,

    /// Print the path of the config file that would be loaded.
    Path,
}

#[derive(Args, Debug)]
pub struct StartArgs {
    /// Name of the host Zellij session.
    #[arg(short = 's', long, default_value = "verij")]
    pub session_name: String,

    /// Path to a literal KDL layout file (overrides user Verij template).
    #[arg(short = 'l', long)]
    pub layout: Option<PathBuf>,

    /// Path to verij_plugin.wasm (overrides automatic search).
    #[arg(short = 'p', long)]
    pub plugin_path: Option<PathBuf>,

    /// Override [workspace].sidebar_width for this new host (e.g. "25%").
    #[arg(long)]
    pub sidebar_width: Option<String>,

    /// Fail with an error if the host session already exists instead of attaching.
    #[arg(long)]
    pub no_attach: bool,
}

#[derive(Args, Debug)]
pub struct AttachArgs {
    /// Name of the host session to attach to.
    #[arg(default_value = "verij")]
    pub session_name: String,

    /// Create and start the session if it does not exist.
    #[arg(short = 'c', long)]
    pub create: bool,
}

// ---------------------------------------------------------------------------
// Subcommand handlers
// ---------------------------------------------------------------------------

fn verify_host(name: &str) -> Result<()> {
    if registry::marker_key(name)?.is_some() {
        if session::is_verij_host(name)? {
            return Ok(());
        }
        bail!("Zellij session '{name}' is registered as a Verij host but its sidebar is missing. Refusing to attach an unrelated session.");
    }
    bail!("'{name}' is not a registered Verij host. Do not rename hosts with Zellij session manager; use Verij's Rename Host action.")
}

fn effective_sidebar_width(cli: Option<&str>, cfg: &config::Config) -> String {
    cli.unwrap_or(&cfg.workspace.sidebar_width).to_string()
}

fn layout_for_new_host(args: &StartArgs, cfg: &config::Config, host_name: &str) -> Result<PathBuf> {
    if let Some(custom) = &args.layout {
        if !custom.exists() {
            bail!("Specified layout file does not exist: {}", custom.display());
        }
        return Ok(custom.clone());
    }

    layout::resolve_plugin_path(args.plugin_path.as_deref())?;
    let layout_config = layout::LayoutConfig {
        verij_bin: layout::resolve_verij_bin(),
        sidebar_size: effective_sidebar_width(args.sidebar_width.as_deref(), cfg),
    };
    layout::write_layout_file(&layout_config, host_name)
}

fn handle_start(args: StartArgs) -> Result<()> {
    let cfg = config::Config::load();
    layout::init_user_layout()?;
    let host_name = args.session_name.as_str();

    match session::session_status(&host_name)? {
        session::SessionStatus::Live | session::SessionStatus::Exited => {
            verify_host(host_name)?;
            if args.no_attach {
                bail!(
                    "Session '{}' already exists. Specify a different name with --session-name or attach with 'verij attach'.",
                    host_name
                );
            }

            eprintln!(
                "Session '{}' already exists. Attaching to it...",
                host_name
            );
            session::restore_last_inner_session(&host_name)?;
            return session::attach_session(&host_name);
        }
        session::SessionStatus::Missing => {}
    }

    let layout_path = layout_for_new_host(&args, &cfg, host_name)?;

    registry::register(host_name)?;
    session::restore_last_inner_session(host_name)?;
    session::start_host_session(
        host_name,
        &layout_path,
        args.no_attach,
        cfg.host.pane_frame_style,
        cfg.host.focus_follows_mouse,
    )
}

fn handle_attach(args: AttachArgs) -> Result<()> {
    let host_name = args.session_name.as_str();
    let status = session::session_status(&host_name)?;

    if matches!(
        status,
        session::SessionStatus::Live | session::SessionStatus::Exited
    ) {
        verify_host(host_name)?;
        session::restore_last_inner_session(host_name)?;
        session::attach_session(host_name)
    } else if args.create {
        handle_start(StartArgs {
            session_name: args.session_name,
            layout: None,
            plugin_path: None,
            sidebar_width: None,
            no_attach: false,
        })
    } else {
        let active_sessions = session::list_sessions()?;
        let active_str = if active_sessions.is_empty() {
            "none".to_string()
        } else {
            active_sessions.join(", ")
        };

        bail!(
            "Host session '{}' is not running.\nActive sessions: {}\nRun 'verij start --session-name {}' or 'verij attach -c {}' to create it.",
            host_name,
            active_str,
            args.session_name,
            args.session_name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_width_uses_cli_then_config_then_default() {
        let defaults = config::Config::default();
        assert_eq!(effective_sidebar_width(None, &defaults), "25%");
        let config: config::Config = toml::from_str("[workspace]\nsidebar_width = '32%'\n").unwrap();
        assert_eq!(effective_sidebar_width(None, &config), "32%");
        assert_eq!(effective_sidebar_width(Some("18%"), &config), "18%");
    }

    #[test]
    fn explicit_layout_bypasses_rendering_and_plugin_search() {
        let path = std::env::temp_dir().join(format!(
            "verij-literal-{}-{}.kdl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "layout { tab name=\"custom\" }\n").unwrap();
        let args = StartArgs {
            session_name: "test".into(),
            layout: Some(path.clone()),
            plugin_path: Some(PathBuf::from("/missing/verij_plugin.wasm")),
            sidebar_width: Some("18%".into()),
            no_attach: false,
        };
        assert_eq!(layout_for_new_host(&args, &config::Config::default(), "test").unwrap(), path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "layout { tab name=\"custom\" }\n");
        std::fs::remove_file(path).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => handle_start(args),
        Commands::Attach(args) => handle_attach(args),
        Commands::Ui => {
            if let Ok(host) = std::env::var("ZELLIJ_SESSION_NAME") {
                if let Some(key) = registry::marker_key(&host)? {
                    std::env::set_var("VERIJ_HOST_MARKER_KEY", key);
                    std::env::set_var("VERIJ_HOST_NAME", host);
                }
            }
            let cfg = config::Config::load();
            tui::run(cfg).await
        }
        Commands::Config(cmd) => match cmd {
            ConfigCommands::Init => config::write_default_config(),
            ConfigCommands::Path => {
                match config::config_path() {
                    Some(p) => println!("{}", p.display()),
                    None => println!("(cannot determine config path)"),
                }
                Ok(())
            }
        },
    }
}
