//! HTTP Source - Webhook Receiver
//!
//! A Source that receives HTTP POST requests and emits `http.request` events.
//! Supports optional HMAC signature validation for webhook security.
//!
//! Sources are SILENT - they only produce domain messages.
//! All lifecycle events are published by the engine.
//!
//! # Usage
//!
//! ```bash
//! # Start with default settings (port 8080, path /)
//! http-source
//!
//! # Custom port and path
//! http-source --port 3000 --path /webhook
//!
//! # With HMAC signature validation
//! http-source --secret my-secret-key
//! ```

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::any,
    Router,
};
use clap::Parser;
use emergent_client::{EmergentMessage, EmergentSource};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::signal::unix::{signal, SignalKind};

/// HTTP webhook receiver that emits http.request events.
#[derive(Parser, Debug, Clone)]
#[command(name = "http-source")]
#[command(about = "Receives HTTP webhooks and emits events")]
struct Args {
    /// Port to listen on.
    #[arg(short, long, env = "HTTP_SOURCE_PORT", default_value = "8080")]
    port: u16,

    /// Host to bind to.
    #[arg(long, env = "HTTP_SOURCE_HOST", default_value = "0.0.0.0")]
    host: String,

    /// Path to accept requests on.
    #[arg(long, env = "HTTP_SOURCE_PATH", default_value = "/")]
    path: String,

    /// Optional HMAC secret for signature validation.
    /// If provided, requests must include X-Signature header with HMAC-SHA256.
    #[arg(long, env = "HTTP_SOURCE_SECRET")]
    secret: Option<String>,
}

/// Payload for http.request events.
#[derive(Debug, serde::Serialize)]
struct HttpRequestPayload {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
    remote_addr: Option<String>,
}

/// Shared application state.
struct AppState {
    source: Arc<EmergentSource>,
    secret: Option<String>,
}

/// Validates HMAC-SHA256 signature.
fn validate_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(body);

    let expected = match hex::decode(signature.trim_start_matches("sha256=")) {
        Ok(h) => h,
        Err(_) => return false,
    };

    mac.verify_slice(&expected).is_ok()
}

/// Handles incoming HTTP requests.
async fn handle_request(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Validate signature if secret is configured
    if let Some(ref secret) = state.secret {
        if let Some(signature) = headers.get("x-signature").and_then(|h| h.to_str().ok()) {
            if !validate_signature(secret, &body, signature) {
                return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
            }
        } else {
            return (StatusCode::UNAUTHORIZED, "Missing signature").into_response();
        }
    }

    // Convert headers to HashMap
    let headers_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|val| (k.as_str().to_string(), val.to_string()))
        })
        .collect();

    // Convert body to string
    let body_str = String::from_utf8_lossy(&body).to_string();

    // Create payload
    let payload = HttpRequestPayload {
        method: method.to_string(),
        path: "/".to_string(), // Axum doesn't provide path in handler
        headers: headers_map,
        body: body_str,
        remote_addr: None,
    };

    // Create and publish message
    let message = EmergentMessage::new("http.request").with_payload(json!(payload));

    match state.source.publish(message).await {
        Ok(()) => (StatusCode::ACCEPTED, "").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to publish event").into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the source name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "http-source".to_string());

    // Connect to the Emergent engine (silently - lifecycle events come from engine)
    let source = match EmergentSource::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Create shared state
    let state = Arc::new(AppState {
        source: Arc::new(source),
        secret: args.secret.clone(),
    });

    // Create router
    let app = Router::new()
        .route(&args.path, any(handle_request))
        .with_state(state.clone());

    // Parse socket address
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Create server with graceful shutdown
    let server = axum::serve(
        tokio::net::TcpListener::bind(&addr).await?,
        app.into_make_service(),
    );

    // Run server with shutdown signal
    tokio::select! {
        result = server => {
            result?;
        }
        _ = sigterm.recv() => {
            let _ = state.source.disconnect().await;
        }
    }

    Ok(())
}
