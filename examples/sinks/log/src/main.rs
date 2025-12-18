//! Log Sink Example
//!
//! A Sink that subscribes to events and logs them to a file.
//! Demonstrates the Sink pattern in Emergent.
//!
//! Sinks are SILENT on the console - they handle egress to their destination.
//! This sink writes to a file; the console_sink handles console output.
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
#[command(about = "Logs events to a file")]
struct Args {
    /// Output log file path.
    #[arg(short, long, default_value = "./timer_events.log")]
    output: PathBuf,

    /// Override subscription types (comma-separated).
    /// By default, queries the engine for configured subscriptions.
    ///
    /// Note: The SDK automatically handles `system.shutdown` for graceful shutdown.
    #[arg(short, long)]
    subscribe: Option<String>,
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

    // Get the sink name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "log_sink".to_string());

    // Connect to the Emergent engine
    // The SDK automatically handles system.shutdown for graceful shutdown
    let sink = match EmergentSink::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Get subscription topics: from command line or query engine
    let topics: Vec<String> = if let Some(ref subscribe) = args.subscribe {
        // Use command line override
        subscribe.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        // Query engine for configured subscriptions
        match sink.get_my_subscriptions().await {
            Ok(subs) => subs,
            Err(e) => {
                eprintln!("Failed to get subscriptions from engine: {e}");
                std::process::exit(1);
            }
        }
    };

    // Open log file for appending
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.output)?;

    // Write startup marker to file (not console)
    let startup_line = format!(
        "[{}] === Log Sink started, subscribing to: {:?} ===\n",
        Utc::now().format("%Y-%m-%d %H:%M:%S%.3f UTC"),
        topics
    );
    log_file.write_all(startup_line.as_bytes())?;
    log_file.flush()?;

    // Subscribe to configured message types
    let topics_refs: Vec<&str> = topics.iter().map(|s| s.as_str()).collect();
    let mut stream = match sink.subscribe(&topics_refs).await {
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
                let _ = sink.disconnect().await;
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
                            payload: serde_json::to_string(msg.payload()).unwrap_or_else(|_| "{}".to_string()),
                        };

                        // Write to log file (silently - no console output)
                        let log_line = entry.to_log_line();
                        let _ = log_file.write_all(log_line.as_bytes());
                        let _ = log_file.flush();
                    }
                    None => {
                        // Stream ended (graceful shutdown)
                        break;
                    }
                }
            }
        }
    }

    // Write shutdown marker to file
    let shutdown_line = format!(
        "[{}] === Log Sink stopped ===\n",
        Utc::now().format("%Y-%m-%d %H:%M:%S%.3f UTC")
    );
    let _ = log_file.write_all(shutdown_line.as_bytes());
    let _ = log_file.flush();

    Ok(())
}
