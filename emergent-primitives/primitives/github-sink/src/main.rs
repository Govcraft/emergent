//! GitHub Sink - GitHub API Client (Stub)
//!
//! This is a stub implementation that will be completed in a future release.

use clap::Parser;

/// GitHub API client (stub implementation).
#[derive(Parser, Debug)]
#[command(name = "github-sink")]
#[command(about = "Interacts with GitHub API (not yet implemented)")]
struct Args {
    /// GitHub personal access token.
    #[arg(short, long, env = "GITHUB_SINK_TOKEN")]
    token: String,

    /// Repository owner.
    #[arg(short, long, env = "GITHUB_SINK_OWNER")]
    owner: String,

    /// Repository name.
    #[arg(short, long, env = "GITHUB_SINK_REPO")]
    repo: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _args = Args::parse();

    eprintln!("github-sink is not yet implemented.");
    eprintln!("This primitive will be completed in a future release.");

    std::process::exit(1);
}
