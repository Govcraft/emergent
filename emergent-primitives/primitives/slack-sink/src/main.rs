//! Slack Sink - Slack Message Poster (Stub)
//!
//! This is a stub implementation that will be completed in a future release.

use clap::Parser;

/// Slack message poster (stub implementation).
#[derive(Parser, Debug)]
#[command(name = "slack-sink")]
#[command(about = "Posts messages to Slack (not yet implemented)")]
struct Args {
    /// Slack bot token.
    #[arg(short, long, env = "SLACK_SINK_TOKEN")]
    token: String,

    /// Default channel to post to.
    #[arg(short, long, env = "SLACK_SINK_DEFAULT_CHANNEL")]
    default_channel: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _args = Args::parse();

    eprintln!("slack-sink is not yet implemented.");
    eprintln!("This primitive will be completed in a future release.");

    std::process::exit(1);
}
