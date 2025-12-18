//! Filter Handler Example
//!
//! A Handler that subscribes to `timer.tick` events and publishes `timer.filtered`
//! events for every Nth tick. Demonstrates the Handler pattern in Emergent.
//!
//! Handlers are SILENT - they only produce domain messages.
//! All lifecycle events are published by the engine.
//!
//! # Messages Published
//!
//! - `timer.filtered` - For ticks that pass the filter
//! - `filter.processed` - Status of each tick (passed or filtered)
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

/// Payload for filter.processed domain events.
#[derive(Debug, Serialize)]
struct FilterProcessedPayload {
    /// Original sequence number from timer.tick.
    original_sequence: u64,
    /// Action taken: "passed" or "filtered".
    action: String,
    /// Reason for the action.
    reason: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the handler name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "filter_handler".to_string());

    // Connect to the Emergent engine
    // The SDK automatically handles system.shutdown for graceful shutdown
    let handler = match EmergentHandler::connect(&name).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Subscribe to timer.tick events
    // The SDK automatically handles system.shutdown for graceful shutdown
    let mut stream = match handler.subscribe(&["timer.tick"]).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Process incoming messages
    // The SDK automatically handles system.shutdown and closes the stream gracefully
    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                let _ = handler.disconnect().await;
                break;
            }

            // Handle incoming messages
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        // Parse the timer tick payload
                        let tick: TimerTickPayload = match msg.payload_as() {
                            Ok(t) => t,
                            Err(_) => continue,
                        };

                        // Check if this tick passes the filter
                        if tick.sequence.is_multiple_of(args.filter_every) {
                            // Publish timer.filtered (the domain event for passed ticks)
                            let filtered_payload = FilteredPayload {
                                original_sequence: tick.sequence,
                                original_interval_ms: tick.interval_ms,
                                filter_reason: format!("every_{}th", args.filter_every),
                                filter_every: args.filter_every,
                            };

                            let output = EmergentMessage::new("timer.filtered")
                                .with_causation_id(msg.id())
                                .with_payload(json!(filtered_payload));

                            let _ = handler.publish(output).await;

                            // Publish filter.processed status event
                            let processed = FilterProcessedPayload {
                                original_sequence: tick.sequence,
                                action: "passed".to_string(),
                                reason: format!("every_{}th", args.filter_every),
                            };

                            let status = EmergentMessage::new("filter.processed")
                                .with_causation_id(msg.id())
                                .with_payload(json!(processed));

                            let _ = handler.publish(status).await;
                        } else {
                            // Publish filter.processed for filtered ticks
                            let processed = FilterProcessedPayload {
                                original_sequence: tick.sequence,
                                action: "filtered".to_string(),
                                reason: format!("not_multiple_of_{}", args.filter_every),
                            };

                            let status = EmergentMessage::new("filter.processed")
                                .with_causation_id(msg.id())
                                .with_payload(json!(processed));

                            let _ = handler.publish(status).await;
                        }
                    }
                    None => {
                        // Stream ended (graceful shutdown)
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
