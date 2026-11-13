#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "wmuxd", version, about = "wmux daemon")]
struct Cli {
    /// Run in the foreground instead of detaching. Logs stream to stderr.
    #[arg(long)]
    foreground: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // The daemon never writes to its own stdout — see CLAUDE.md hard rule 4.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(foreground = cli.foreground, "wmuxd starting (M0 skeleton)");
    Ok(())
}
