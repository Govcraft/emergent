//! Timer Source Example
//!
//! A Source that emits `timer.tick` events at a configurable interval.
//! Demonstrates the Source pattern in Emergent.
//!
//! Sources are SILENT - they only produce domain messages.
//! All lifecycle events are published by the engine.
//!
//! # Usage
//!
//! ```bash
//! # Start with default 5 second interval
//! timer_source
//!
//! # Start with custom interval
//! timer_source --interval 1000
//! ```

use clap::Parser;
use emergent_client::{EmergentMessage, EmergentSource};
use serde_json::json;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

/// Timer Source - emits periodic tick events.
#[derive(Parser, Debug)]
#[command(name = "timer_source")]
#[command(about = "Emits timer.tick events at a configurable interval")]
struct Args {
    /// Interval in milliseconds between ticks.
    #[arg(short, long, default_value = "5000")]
    interval: u64,

    /// Maximum number of ticks to emit (0 = unlimited).
    #[arg(short, long, default_value = "0")]
    max_ticks: u64,
}

/// Payload for timer.tick events.
#[derive(Debug, serde::Serialize)]
struct TimerTickPayload {
    /// Sequence number of this tick.
    sequence: u64,
    /// Interval in milliseconds.
    interval_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the source name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "timer_source".to_string());

    // Connect to the Emergent engine (silently - lifecycle events come from engine)
    let source = match EmergentSource::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Create interval timer
    let mut interval = tokio::time::interval(Duration::from_millis(args.interval));
    let mut sequence: u64 = 0;

    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                let _ = source.disconnect().await;
                break;
            }

            // Handle interval tick
            _ = interval.tick() => {
                sequence += 1;

                // Create the tick message
                let payload = TimerTickPayload {
                    sequence,
                    interval_ms: args.interval,
                };

                let message = EmergentMessage::new("timer.tick").with_payload(json!(payload));

                // Publish the tick (silently - errors are ignored, sink will handle output)
                let _ = source.publish(message).await;

                // Check if we've reached max ticks
                if args.max_ticks > 0 && sequence >= args.max_ticks {
                    let _ = source.disconnect().await;
                    break;
                }
            }
        }
    }

    Ok(())
}
