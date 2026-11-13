#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "wmux",
    version,
    about = "wmux client — a Windows terminal multiplexer"
)]
struct Cli {
    // Subcommands (attach, ls, new-session, …) land in M7.
}

fn main() -> Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
