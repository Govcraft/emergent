//! GitHub Source - GitHub Webhook Receiver (Stub)
//!
//! This is a stub implementation that will be completed in a future release.

use clap::Parser;

/// GitHub webhook receiver (stub implementation).
#[derive(Parser, Debug)]
#[command(name = "github-source")]
#[command(about = "Receives GitHub webhooks (not yet implemented)")]
struct Args {
    /// Webhook secret for signature validation.
    #[arg(short, long, env = "GITHUB_SOURCE_WEBHOOK_SECRET")]
    webhook_secret: String,

    /// Port to listen on.
    #[arg(short, long, env = "GITHUB_SOURCE_PORT", default_value = "8080")]
    port: u16,

    /// Path to accept webhooks on.
    #[arg(long, env = "GITHUB_SOURCE_PATH", default_value = "/webhook")]
    path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _args = Args::parse();

    eprintln!("github-source is not yet implemented.");
    eprintln!("This primitive will be completed in a future release.");

    std::process::exit(1);
}
