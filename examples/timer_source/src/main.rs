//! Timer Source Example
//!
//! A Source that emits `timer.tick` events at a configurable interval.
//! Demonstrates the Source pattern in Emergent.
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
use tokio::signal::unix::{SignalKind, signal};

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

    // println!("Timer Source starting...");
    // println!("  Name: {name}");
    // println!("  Interval: {}ms", args.interval);
    // if args.max_ticks > 0 {
    //     println!("  Max ticks: {}", args.max_ticks);
    // }

    // Connect to the Emergent engine
    let source = match EmergentSource::connect(&name).await {
        Ok(s) => {
            // println!("Connected to Emergent engine");
            s
        }
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            eprintln!("Make sure the engine is running and EMERGENT_SOCKET is set.");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Create interval timer
    let mut interval = tokio::time::interval(Duration::from_millis(args.interval));
    let mut sequence: u64 = 0;

    println!("Starting to emit timer.tick events...");

    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                // println!("Received SIGTERM, disconnecting gracefully...");
                if let Err(e) = source.disconnect().await {
                    eprintln!("Error during disconnect: {e}");
                }
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

                // Publish the tick
                match source.publish(message).await {
                    Ok(()) => {
                        // println!("[tick #{}] Published timer.tick", sequence);
                    }
                    Err(_e) => {
                        // eprintln!("[tick #{}] Failed to publish: {}", sequence, e);
                    }
                }

                // Check if we've reached max ticks
                if args.max_ticks > 0 && sequence >= args.max_ticks {
                    // println!("Reached max ticks ({}), disconnecting...", args.max_ticks);
                    if let Err(e) = source.disconnect().await {
                        eprintln!("Error during disconnect: {e}");
                    }
                    break;
                }
            }
        }
    }

    // println!("Timer Source stopped.");
    Ok(())
}
