//! Log Sink Example
//!
//! A Sink that subscribes to `timer.filtered` events and logs them to a file.
//! Demonstrates the Sink pattern in Emergent.
//!
//! # Usage
//!
//! ```bash
//! # Log to default file (./timer_events.log)
//! log_sink
//!
//! # Log to custom file
//! log_sink --output /var/log/timer_events.log
//! ```

use chrono::{DateTime, Utc};
use clap::Parser;
use emergent_client::EmergentSink;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use tokio::signal::unix::{signal, SignalKind};

/// Log Sink - writes received events to a log file.
#[derive(Parser, Debug)]
#[command(name = "log_sink")]
#[command(about = "Logs timer.filtered events to a file")]
struct Args {
    /// Output log file path.
    #[arg(short, long, default_value = "./timer_events.log")]
    output: PathBuf,

    /// Message types to subscribe to (comma-separated).
    #[arg(short, long, default_value = "timer.filtered")]
    subscribe: String,
}

/// Log entry format written to the file.
struct LogEntry {
    timestamp: DateTime<Utc>,
    message_id: String,
    message_type: String,
    source: String,
    payload: String,
}

impl LogEntry {
    fn to_log_line(&self) -> String {
        format!(
            "[{}] id={} type={} source={} payload={}\n",
            self.timestamp.format("%Y-%m-%d %H:%M:%S%.3f UTC"),
            self.message_id,
            self.message_type,
            self.source,
            self.payload
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Parse subscription topics
    let topics: Vec<&str> = args.subscribe.split(',').map(|s| s.trim()).collect();

    // Get the sink name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "log_sink".to_string());

    println!("Log Sink starting...");
    println!("  Name: {name}");
    println!("  Output: {}", args.output.display());
    println!("  Subscribing to: {:?}", topics);

    // Open log file for appending
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.output)?;

    // Write startup marker
    let startup_line = format!(
        "[{}] === Log Sink started, subscribing to: {:?} ===\n",
        Utc::now().format("%Y-%m-%d %H:%M:%S%.3f UTC"),
        topics
    );
    log_file.write_all(startup_line.as_bytes())?;
    log_file.flush()?;

    // Connect to the Emergent engine
    let sink = match EmergentSink::connect(&name).await {
        Ok(s) => {
            println!("Connected to Emergent engine");
            s
        }
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            eprintln!("Make sure the engine is running and EMERGENT_SOCKET is set.");
            std::process::exit(1);
        }
    };

    // Subscribe to configured message types
    let mut stream = match sink.subscribe(&topics).await {
        Ok(s) => {
            println!("Subscribed to message types");
            s
        }
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    println!("Waiting for messages...");

    // Process incoming messages
    loop {
        tokio::select! {
            // Handle SIGTERM for graceful shutdown
            _ = sigterm.recv() => {
                println!("Received SIGTERM, disconnecting gracefully...");
                if let Err(e) = sink.disconnect().await {
                    eprintln!("Error during disconnect: {e}");
                }
                break;
            }

            // Handle incoming messages
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let timestamp = Utc::now();

                        // Create log entry
                        let entry = LogEntry {
                            timestamp,
                            message_id: msg.id().to_string(),
                            message_type: msg.message_type().to_string(),
                            source: msg.source().to_string(),
                            payload: serde_json::to_string(&msg.payload()).unwrap_or_else(|_| "{}".to_string()),
                        };

                        // Write to log file
                        let log_line = entry.to_log_line();
                        if let Err(e) = log_file.write_all(log_line.as_bytes()) {
                            eprintln!("Failed to write to log file: {e}");
                        } else if let Err(e) = log_file.flush() {
                            eprintln!("Failed to flush log file: {e}");
                        }

                        // Also print to console for visibility
                        println!(
                            "[{}] Logged: {} from {} (seq: {})",
                            timestamp.format("%H:%M:%S"),
                            msg.message_type(),
                            msg.source(),
                            msg.payload()
                                .get("original_sequence")
                                .and_then(|v| v.as_u64())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "?".to_string())
                        );
                    }
                    None => {
                        println!("Stream ended, exiting...");
                        break;
                    }
                }
            }
        }
    }

    // Write shutdown marker
    let shutdown_line = format!(
        "[{}] === Log Sink stopped ===\n",
        Utc::now().format("%Y-%m-%d %H:%M:%S%.3f UTC")
    );
    log_file.write_all(shutdown_line.as_bytes())?;
    log_file.flush()?;

    println!("Log Sink stopped.");
    Ok(())
}
