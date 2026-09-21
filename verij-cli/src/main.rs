/// verij-cli/src/main.rs
///
/// Entry point for the `verij` binary.
///
/// Dispatches CLI subcommands via clap. Currently supports:
///   - `verij ui` — launch the Ratatui TUI sidebar.
///
/// More subcommands (e.g. `verij start`, `verij new`) will be added in
/// future phases.
use anyhow::Result;
use clap::{Parser, Subcommand};

mod actions;
mod pipe_reader;
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
    /// Launch the interactive TUI sidebar.
    ///
    /// Connects to the verij-plugin via a Zellij native pipe and renders a
    /// live, navigable 2-level tree of sessions and tabs.
    Ui,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ui => tui::run().await,
    }
}
