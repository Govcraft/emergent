//! Console Sink - Plain Text Output
//!
//! A Sink that subscribes to domain and system events and displays them
//! on the console in plain text format.
//!
//! This sink is silent on startup/shutdown - it only outputs received messages.
//!
//! # Usage
//!
//! ```bash
//! # Subscribe to default topics
//! console_sink
//!
//! # Subscribe to custom topics
//! console_sink --subscribe "timer.tick,system.started.*"
//! ```

use chrono::Utc;
use clap::Parser;
use emergent_client::EmergentSink;
use serde::Deserialize;
use tokio::signal::unix::{signal, SignalKind};

/// Console Sink - displays events in plain text format.
#[derive(Parser, Debug)]
#[command(name = "console_sink")]
#[command(about = "Plain console output for Emergent events")]
struct Args {
    /// Message types to subscribe to (comma-separated).
    /// Supports wildcards like "system.started.*"
    #[arg(
        short,
        long,
        default_value = "timer.filtered,filter.processed,system.started.*,system.stopped.*,system.error.*"
    )]
    subscribe: String,
}

/// Payload for system events.
#[derive(Debug, Deserialize)]
struct SystemEventPayload {
    name: String,
    kind: String,
    #[serde(default)]
    error: Option<String>,
}

/// Payload for filter.processed events.
#[derive(Debug, Deserialize)]
struct FilterProcessedPayload {
    original_sequence: u64,
    action: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Payload for timer.filtered events.
#[derive(Debug, Deserialize)]
struct TimerFilteredPayload {
    original_sequence: u64,
    #[serde(default)]
    filter_reason: Option<String>,
    #[serde(default)]
    filter_every: Option<u64>,
}

/// Format a message for console output.
fn format_message(message_type: &str, payload: &serde_json::Value) -> String {
    // Try to format based on message type
    if message_type.starts_with("system.started.") {
        if let Ok(p) = serde_json::from_value::<SystemEventPayload>(payload.clone()) {
            return format!("[STARTED] {} ({})", p.name, p.kind);
        }
    } else if message_type.starts_with("system.stopped.") {
        if let Ok(p) = serde_json::from_value::<SystemEventPayload>(payload.clone()) {
            return format!("[STOPPED] {} ({})", p.name, p.kind);
        }
    } else if message_type.starts_with("system.error.") {
        if let Ok(p) = serde_json::from_value::<SystemEventPayload>(payload.clone()) {
            let error_msg = p.error.as_deref().unwrap_or("unknown error");
            return format!("[ERROR] {} ({}): {}", p.name, p.kind, error_msg);
        }
    } else if message_type == "filter.processed" {
        if let Ok(p) = serde_json::from_value::<FilterProcessedPayload>(payload.clone()) {
            let reason = p.reason.as_deref().unwrap_or("");
            return format!(
                "[FILTER] tick #{} {} {}",
                p.original_sequence, p.action, reason
            )
            .trim()
            .to_string();
        }
    } else if message_type == "timer.filtered"
        && let Ok(p) = serde_json::from_value::<TimerFilteredPayload>(payload.clone())
    {
        let filter_info = match (p.filter_reason.as_deref(), p.filter_every) {
            (Some(reason), Some(every)) => format!("({} every {})", reason, every),
            (Some(reason), None) => format!("({})", reason),
            (None, Some(every)) => format!("(every {})", every),
            (None, None) => String::new(),
        };
        return format!("[FILTERED] tick #{} {}", p.original_sequence, filter_info)
            .trim()
            .to_string();
    }

    // Fallback: just show the message type and raw payload
    format!("[{}] {}", message_type, payload)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Parse subscription topics
    let topics: Vec<&str> = args.subscribe.split(',').map(|s| s.trim()).collect();

    // Get the sink name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "console_sink".to_string());

    // Connect to the Emergent engine (silently - no startup messages)
    let sink = match EmergentSink::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            eprintln!("Make sure the engine is running and EMERGENT_SOCKET is set.");
            std::process::exit(1);
        }
    };

    // Subscribe to configured message types
    let mut stream = match sink.subscribe(&topics).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Process incoming messages
    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                // Silent shutdown
                if let Err(e) = sink.disconnect().await {
                    eprintln!("Error during disconnect: {e}");
                }
                break;
            }

            // Handle incoming messages
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let timestamp = Utc::now().format("%H:%M:%S%.3f");
                        let formatted = format_message(msg.message_type(), msg.payload());
                        println!("[{}] {}", timestamp, formatted);
                    }
                    None => {
                        // Stream ended, exit silently
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
