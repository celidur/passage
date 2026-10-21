use anyhow::Result;
use clap::{Parser, Subcommand};
use passage::{commands, config::Config};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Passage — a bare-metal reverse proxy manager")]
struct Cli {
    #[arg(short, long, default_value = "passage.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the data directory and administrator token. Never overwrites an existing token.
    Init,
    /// Validate configuration and stored routes without starting listeners.
    Check,
    /// Start the management interface and Pingora proxy.
    Serve,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Command::Init => commands::init(&config),
        Command::Check => commands::check(&config),
        Command::Serve => commands::serve(config),
    }
}
