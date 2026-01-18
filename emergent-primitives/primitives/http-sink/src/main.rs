//! HTTP Sink - Outbound HTTP Client
//!
//! A Sink that subscribes to events and makes HTTP requests based on message payloads.
//! Supports configurable retries, timeouts, and authentication.
//!
//! Sinks are SILENT - they only consume messages.
//! All lifecycle events are published by the engine.
//!
//! # Usage
//!
//! ```bash
//! # Make requests without base URL
//! http-sink
//!
//! # With base URL prefix
//! http-sink --base-url https://api.example.com
//!
//! # With authentication and retries
//! http-sink --auth-header "Bearer token123" --retries 5 --timeout 60
//! ```
//!
//! # Message Payload Format
//!
//! The sink expects messages with payload containing:
//! - `url` or `path` - target URL (path is prefixed with base-url if provided)
//! - `method` - HTTP method (GET, POST, etc.) - defaults to POST
//! - `headers` - optional headers object
//! - `body` - optional request body

use clap::Parser;
use emergent_client::EmergentSink;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

/// HTTP client that makes outbound requests from events.
#[derive(Parser, Debug, Clone)]
#[command(name = "http-sink")]
#[command(about = "Makes HTTP requests from consumed events")]
struct Args {
    /// Base URL to prepend to relative paths.
    #[arg(short, long, env = "HTTP_SINK_BASE_URL")]
    base_url: Option<String>,

    /// Request timeout in seconds.
    #[arg(short, long, env = "HTTP_SINK_TIMEOUT", default_value = "30")]
    timeout: u64,

    /// Number of retries on failure.
    #[arg(short, long, env = "HTTP_SINK_RETRIES", default_value = "3")]
    retries: u32,

    /// Optional authorization header value.
    #[arg(long, env = "HTTP_SINK_AUTH_HEADER")]
    auth_header: Option<String>,
}

/// Extracts URL from message payload.
fn extract_url(payload: &Value, base_url: &Option<String>) -> Option<String> {
    if let Some(url) = payload.get("url").and_then(|u| u.as_str()) {
        return Some(url.to_string());
    }

    if let Some(path) = payload.get("path").and_then(|p| p.as_str()) {
        if let Some(base) = base_url {
            return Some(format!("{}{}", base.trim_end_matches('/'), path));
        }
        return Some(path.to_string());
    }

    None
}

/// Makes an HTTP request with retries.
async fn make_request(
    client: &Client,
    url: &str,
    method: &str,
    headers: &Value,
    body: &Value,
    args: &Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut attempts = 0;
    let max_attempts = args.retries + 1;

    while attempts < max_attempts {
        attempts += 1;

        let mut request = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            "PATCH" => client.patch(url),
            _ => client.post(url),
        };

        // Add auth header if configured
        if let Some(ref auth) = args.auth_header {
            request = request.header("Authorization", auth);
        }

        // Add custom headers from payload
        if let Some(headers_obj) = headers.as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    request = request.header(key, val_str);
                }
            }
        }

        // Add body if not null
        if !body.is_null() {
            request = request.json(body);
        }

        // Execute request
        match request.send().await {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(());
                } else if attempts < max_attempts {
                    eprintln!(
                        "Request failed with status {}, retrying ({}/{})",
                        response.status(),
                        attempts,
                        max_attempts
                    );
                    tokio::time::sleep(Duration::from_millis(100 * u64::from(attempts))).await;
                } else {
                    return Err(format!("Request failed with status: {}", response.status()).into());
                }
            }
            Err(e) => {
                if attempts < max_attempts {
                    eprintln!("Request error: {e}, retrying ({}/{max_attempts})", attempts);
                    tokio::time::sleep(Duration::from_millis(100 * u64::from(attempts))).await;
                } else {
                    return Err(e.into());
                }
            }
        }
    }

    Err("Max retries exceeded".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the sink name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "http-sink".to_string());

    // Connect to the Emergent engine
    let sink = match EmergentSink::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            eprintln!("Make sure the engine is running and EMERGENT_SOCKET is set.");
            std::process::exit(1);
        }
    };

    // Get subscription topics from engine
    let topics = match sink.get_my_subscriptions().await {
        Ok(subs) => subs,
        Err(e) => {
            eprintln!("Failed to get subscriptions from engine: {e}");
            std::process::exit(1);
        }
    };

    // Subscribe to configured message types
    let topics_refs: Vec<&str> = topics.iter().map(|s| s.as_str()).collect();
    let mut stream = match sink.subscribe(&topics_refs).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Create HTTP client with timeout
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout))
        .build()?;

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Process incoming messages
    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                let _ = sink.disconnect().await;
                break;
            }

            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let payload = msg.payload();

                        // Extract URL
                        let url = match extract_url(payload, &args.base_url) {
                            Some(u) => u,
                            None => {
                                eprintln!("Message missing 'url' or 'path' field in payload");
                                continue;
                            }
                        };

                        // Extract method (default to POST)
                        let method = payload
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or("POST");

                        // Extract headers (default to null)
                        let headers = payload.get("headers").unwrap_or(&Value::Null);

                        // Extract body (default to entire payload)
                        let body = payload.get("body").unwrap_or(payload);

                        // Make request with retries
                        if let Err(e) = make_request(&client, &url, method, headers, body, &args).await {
                            eprintln!("Failed to make request to {url}: {e}");
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
