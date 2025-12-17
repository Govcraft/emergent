//! Filter Handler Example
//!
//! A Handler that subscribes to `timer.tick` events and publishes `timer.filtered`
//! events for every Nth tick. Demonstrates the Handler pattern in Emergent.
//!
//! # Usage
//!
//! ```bash
//! # Filter every 5th tick (default)
//! filter_handler
//!
//! # Filter every 10th tick
//! filter_handler --filter-every 10
//! ```

use clap::Parser;
use emergent_client::{EmergentHandler, EmergentMessage};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::signal::unix::{signal, SignalKind};

/// Filter Handler - passes through every Nth timer tick.
#[derive(Parser, Debug)]
#[command(name = "filter_handler")]
#[command(about = "Filters timer.tick events, passing through every Nth tick")]
struct Args {
    /// Pass through every Nth tick.
    #[arg(short, long, default_value = "5")]
    filter_every: u64,
}

/// Payload for incoming timer.tick events.
#[derive(Debug, Deserialize)]
struct TimerTickPayload {
    sequence: u64,
    interval_ms: u64,
}

/// Payload for outgoing timer.filtered events.
#[derive(Debug, Serialize)]
struct FilteredPayload {
    /// Original sequence number from timer.tick.
    original_sequence: u64,
    /// Original interval from timer.tick.
    original_interval_ms: u64,
    /// Reason for passing through filter.
    filter_reason: String,
    /// The filter configuration (every N).
    filter_every: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the handler name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "filter_handler".to_string());

    println!("Filter Handler starting...");
    println!("  Name: {name}");
    println!("  Filter: pass every {}th tick", args.filter_every);

    // Connect to the Emergent engine
    let handler = match EmergentHandler::connect(&name).await {
        Ok(h) => {
            println!("Connected to Emergent engine");
            h
        }
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            eprintln!("Make sure the engine is running and EMERGENT_SOCKET is set.");
            std::process::exit(1);
        }
    };

    // Subscribe to timer.tick events
    let mut stream = match handler.subscribe(&["timer.tick"]).await {
        Ok(s) => {
            println!("Subscribed to timer.tick events");
            s
        }
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    println!("Waiting for timer.tick events...");

    // Process incoming messages
    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                println!("Received SIGTERM, disconnecting gracefully...");
                if let Err(e) = handler.disconnect().await {
                    eprintln!("Error during disconnect: {e}");
                }
                break;
            }

            // Handle incoming messages
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        // Parse the timer tick payload
                        let tick: TimerTickPayload = match msg.payload_as() {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("Failed to parse timer.tick payload: {e}");
                                continue;
                            }
                        };

                        // Check if this tick passes the filter
                        if tick.sequence.is_multiple_of(args.filter_every) {
                            println!(
                                "[tick #{}] PASSED - publishing timer.filtered",
                                tick.sequence
                            );

                            // Create filtered message with causation chain
                            let filtered_payload = FilteredPayload {
                                original_sequence: tick.sequence,
                                original_interval_ms: tick.interval_ms,
                                filter_reason: format!("every_{}th", args.filter_every),
                                filter_every: args.filter_every,
                            };

                            let output = EmergentMessage::new("timer.filtered")
                                .with_causation_id(msg.id())
                                .with_payload(json!(filtered_payload));

                            if let Err(e) = handler.publish(output).await {
                                eprintln!("Failed to publish timer.filtered: {e}");
                            }
                        } else {
                            println!("[tick #{}] filtered out", tick.sequence);
                        }
                    }
                    None => {
                        println!("Stream ended, exiting...");
                        break;
                    }
                }
            }
        }
    }

    println!("Filter Handler stopped.");
    Ok(())
}
