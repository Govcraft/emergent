//! Slack Source - Slack Event Monitor (Stub)
//!
//! This is a stub implementation that will be completed in a future release.

use clap::Parser;

/// Slack event monitor (stub implementation).
#[derive(Parser, Debug)]
#[command(name = "slack-source")]
#[command(about = "Monitors Slack events (not yet implemented)")]
struct Args {
    /// Slack bot token.
    #[arg(short, long, env = "SLACK_SOURCE_TOKEN")]
    token: String,

    /// Comma-separated list of channels to monitor.
    #[arg(short, long, env = "SLACK_SOURCE_CHANNELS")]
    channels: String,

    /// Slack app-level token for socket mode.
    #[arg(short, long, env = "SLACK_SOURCE_APP_TOKEN")]
    app_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _args = Args::parse();

    eprintln!("slack-source is not yet implemented.");
    eprintln!("This primitive will be completed in a future release.");

    std::process::exit(1);
}
