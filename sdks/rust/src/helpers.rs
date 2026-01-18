//! Convenience functions for building Emergent primitives.
//!
//! These helpers eliminate the boilerplate code for connecting, handling signals,
//! and running the event loop. Developers only need to provide their business logic
//! as an async closure.
//!
//! # Examples
//!
//! ## Source with custom logic (interval-based timer)
//!
//! ```rust,ignore
//! use emergent_client::helpers::run_source;
//! use emergent_client::EmergentMessage;
//! use serde_json::json;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_source(Some("my_timer"), |source, mut shutdown| async move {
//!         let mut interval = tokio::time::interval(Duration::from_secs(3));
//!         let mut count = 0u64;
//!
//!         loop {
//!             tokio::select! {
//!                 _ = shutdown.changed() => break,
//!                 _ = interval.tick() => {
//!                     count += 1;
//!                     let msg = EmergentMessage::new("timer.tick")
//!                         .with_payload(json!({"count": count}));
//!                     source.publish(msg).await.map_err(|e| e.to_string())?;
//!                 }
//!             }
//!         }
//!         Ok(())
//!     }).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Source as HTTP webhook
//!
//! ```rust,ignore
//! use emergent_client::helpers::run_source;
//! use emergent_client::EmergentMessage;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_source(Some("webhook"), |source, mut shutdown| async move {
//!         // Start HTTP server, publish on each request
//!         let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
//!
//!         loop {
//!             tokio::select! {
//!                 _ = shutdown.changed() => break,
//!                 result = listener.accept() => {
//!                     let (stream, _) = result?;
//!                     // Parse request, publish message...
//!                     source.publish(EmergentMessage::new("webhook.received")).await?;
//!                 }
//!             }
//!         }
//!         Ok(())
//!     }).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Source as one-shot function
//!
//! ```rust,ignore
//! use emergent_client::helpers::run_source;
//! use emergent_client::EmergentMessage;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_source(Some("one_shot"), |source, _shutdown| async move {
//!         // Run once and exit
//!         source.publish(EmergentMessage::new("startup.complete")).await?;
//!         Ok(())
//!     }).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Handler with message transformation
//!
//! ```rust,ignore
//! use emergent_client::helpers::run_handler;
//! use emergent_client::EmergentMessage;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_handler(
//!         Some("my_handler"),
//!         &["timer.tick"],
//!         |msg, handler| async move {
//!             let output = EmergentMessage::new("timer.processed")
//!                 .with_causation_from_message(msg.id())
//!                 .with_payload(json!({"processed": true}));
//!             handler.publish(output).await.map_err(|e| e.to_string())
//!         }
//!     ).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Sink with message consumption
//!
//! ```rust,ignore
//! use emergent_client::helpers::run_sink;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_sink(
//!         Some("my_sink"),
//!         &["timer.processed"],
//!         |msg| async move {
//!             println!("Received: {:?}", msg.payload());
//!             Ok(())
//!         }
//!     ).await?;
//!     Ok(())
//! }
//! ```

use crate::connection::{EmergentHandler, EmergentSink, EmergentSource};
use crate::message::EmergentMessage;
use std::future::Future;
use thiserror::Error;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;

/// Errors that can occur when running helper functions.
#[derive(Debug, Error)]
pub enum HelperError {
    /// Failed to connect to the Emergent engine.
    #[error("failed to connect to Emergent engine as '{name}': {error}")]
    ConnectionFailed {
        /// The name of the primitive that failed to connect.
        name: String,
        /// The underlying error message.
        error: String,
    },

    /// User-provided function returned an error.
    #[error("user function error: {0}")]
    UserFunction(String),

    /// Failed to publish a message.
    #[error("failed to publish message: {0}")]
    PublishFailed(String),

    /// Failed to subscribe to message types.
    #[error("failed to subscribe: {0}")]
    SubscribeFailed(String),

    /// Failed to set up signal handler.
    #[error("failed to set up signal handler: {0}")]
    SignalHandlerFailed(String),

    /// Failed to disconnect gracefully.
    #[error("failed to disconnect: {0}")]
    DisconnectFailed(String),
}

/// Result type for helper functions.
pub type HelperResult<T> = std::result::Result<T, HelperError>;

/// Default environment variable name for the primitive name.
const EMERGENT_NAME_ENV: &str = "EMERGENT_NAME";

/// Resolve the primitive name from the provided option or environment variable.
fn resolve_name(name: Option<&str>, default: &str) -> String {
    name.map(ToString::to_string)
        .or_else(|| std::env::var(EMERGENT_NAME_ENV).ok())
        .unwrap_or_else(|| default.to_string())
}

/// Shutdown signal receiver type.
///
/// Use this in your source function to check for shutdown signals.
/// Call `.changed().await` to wait for a shutdown signal.
pub type ShutdownReceiver = watch::Receiver<bool>;

/// Run a Source with custom logic.
///
/// This function handles all the boilerplate for running a Source:
/// - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
/// - Connects to the Emergent engine
/// - Sets up SIGTERM signal handling for graceful shutdown
/// - Calls your function with the connected source and a shutdown receiver
/// - Gracefully disconnects after your function completes
///
/// Your function receives:
/// - `source: EmergentSource` - The connected source for publishing messages
/// - `shutdown: ShutdownReceiver` - A watch receiver that signals when shutdown is requested
///
/// # Arguments
///
/// * `name` - Optional name for this source. Falls back to `EMERGENT_NAME` env var,
///   then to the default `"source"`.
/// * `run_fn` - Async function that implements your source logic.
///
/// # Returns
///
/// Returns `Ok(())` on graceful completion or an error if something fails.
///
/// # Example: Interval-based timer
///
/// ```rust,ignore
/// use emergent_client::helpers::run_source;
/// use emergent_client::EmergentMessage;
/// use serde_json::json;
/// use std::time::Duration;
///
/// run_source(Some("my_timer"), |source, mut shutdown| async move {
///     let mut interval = tokio::time::interval(Duration::from_secs(3));
///     let mut count = 0u64;
///
///     loop {
///         tokio::select! {
///             _ = shutdown.changed() => break,
///             _ = interval.tick() => {
///                 count += 1;
///                 let msg = EmergentMessage::new("timer.tick")
///                     .with_payload(json!({"count": count}));
///                 source.publish(msg).await.map_err(|e| e.to_string())?;
///             }
///         }
///     }
///     Ok(())
/// }).await?;
/// ```
///
/// # Example: One-shot source
///
/// ```rust,ignore
/// run_source(Some("init"), |source, _shutdown| async move {
///     source.publish(EmergentMessage::new("system.init")).await?;
///     Ok(())
/// }).await?;
/// ```
pub async fn run_source<F, Fut>(
    name: Option<&str>,
    run_fn: F,
) -> HelperResult<()>
where
    F: FnOnce(EmergentSource, ShutdownReceiver) -> Fut + Send,
    Fut: Future<Output = Result<(), String>> + Send,
{
    let resolved_name = resolve_name(name, "source");

    let source = EmergentSource::connect(&resolved_name).await.map_err(|e| {
        HelperError::ConnectionFailed {
            name: resolved_name.clone(),
            error: e.to_string(),
        }
    })?;

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Set up signal handler
    let mut sigterm =
        signal(SignalKind::terminate()).map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

    // Spawn signal handler task
    let signal_task = tokio::spawn(async move {
        sigterm.recv().await;
        let _ = shutdown_tx.send(true);
    });

    // Run user function
    let result = run_fn(source, shutdown_rx).await;

    // Cancel signal handler task
    signal_task.abort();

    result.map_err(HelperError::UserFunction)
}

/// Run a Handler with message processing.
///
/// This function handles all the boilerplate for running a Handler:
/// - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
/// - Connects to the Emergent engine
/// - Subscribes to the specified message types
/// - Sets up SIGTERM signal handling for graceful shutdown
/// - Runs the message loop, calling your function for each message
/// - Gracefully disconnects on shutdown
///
/// # Arguments
///
/// * `name` - Optional name for this handler. Falls back to `EMERGENT_NAME` env var,
///   then to the default `"handler"`.
/// * `subscriptions` - Message types to subscribe to.
/// * `process_fn` - Async function called for each message with `(msg, &handler)`.
///
/// # Returns
///
/// Returns `Ok(())` on graceful shutdown or an error if something fails.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::helpers::run_handler;
/// use emergent_client::EmergentMessage;
/// use serde_json::json;
///
/// run_handler(
///     Some("my_handler"),
///     &["timer.tick"],
///     |msg, handler| async move {
///         let output = EmergentMessage::new("timer.processed")
///             .with_causation_from_message(msg.id())
///             .with_payload(json!({"processed": true}));
///         handler.publish(output).await.map_err(|e| e.to_string())
///     }
/// ).await?;
/// ```
pub async fn run_handler<F, Fut>(
    name: Option<&str>,
    subscriptions: &[&str],
    process_fn: F,
) -> HelperResult<()>
where
    F: Fn(EmergentMessage, &EmergentHandler) -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), String>> + Send,
{
    let resolved_name = resolve_name(name, "handler");

    let handler = EmergentHandler::connect(&resolved_name)
        .await
        .map_err(|e| HelperError::ConnectionFailed {
            name: resolved_name.clone(),
            error: e.to_string(),
        })?;

    let mut stream = handler
        .subscribe(subscriptions)
        .await
        .map_err(|e| HelperError::SubscribeFailed(e.to_string()))?;

    let mut sigterm =
        signal(SignalKind::terminate()).map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                // Graceful shutdown
                let _ = handler.disconnect().await;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        if let Err(e) = process_fn(msg, &handler).await {
                            return Err(HelperError::UserFunction(e));
                        }
                    }
                    None => {
                        // Stream closed (graceful shutdown from engine)
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Run a Sink with message consumption.
///
/// This function handles all the boilerplate for running a Sink:
/// - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
/// - Connects to the Emergent engine
/// - Subscribes to the specified message types
/// - Sets up SIGTERM signal handling for graceful shutdown
/// - Runs the message loop, calling your function for each message
/// - Gracefully disconnects on shutdown
///
/// # Arguments
///
/// * `name` - Optional name for this sink. Falls back to `EMERGENT_NAME` env var,
///   then to the default `"sink"`.
/// * `subscriptions` - Message types to subscribe to.
/// * `consume_fn` - Async function called for each message with `(msg)`.
///
/// # Returns
///
/// Returns `Ok(())` on graceful shutdown or an error if something fails.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::helpers::run_sink;
///
/// run_sink(
///     Some("my_sink"),
///     &["timer.processed"],
///     |msg| async move {
///         println!("Received: {:?}", msg.payload());
///         Ok(())
///     }
/// ).await?;
/// ```
pub async fn run_sink<F, Fut>(
    name: Option<&str>,
    subscriptions: &[&str],
    consume_fn: F,
) -> HelperResult<()>
where
    F: Fn(EmergentMessage) -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), String>> + Send,
{
    let resolved_name = resolve_name(name, "sink");

    let sink = EmergentSink::connect(&resolved_name)
        .await
        .map_err(|e| HelperError::ConnectionFailed {
            name: resolved_name.clone(),
            error: e.to_string(),
        })?;

    let mut stream = sink
        .subscribe(subscriptions)
        .await
        .map_err(|e| HelperError::SubscribeFailed(e.to_string()))?;

    let mut sigterm =
        signal(SignalKind::terminate()).map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                // Graceful shutdown
                let _ = sink.disconnect().await;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        if let Err(e) = consume_fn(msg).await {
                            return Err(HelperError::UserFunction(e));
                        }
                    }
                    None => {
                        // Stream closed (graceful shutdown from engine)
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_name_with_explicit_name() {
        let name = resolve_name(Some("explicit"), "default");
        assert_eq!(name, "explicit");
    }

    #[test]
    fn resolve_name_with_default() {
        // Clear env var if set
        // SAFETY: This test is run in isolation and we're only modifying
        // an environment variable specific to this crate's tests.
        unsafe {
            std::env::remove_var(EMERGENT_NAME_ENV);
        }
        let name = resolve_name(None, "default");
        assert_eq!(name, "default");
    }

    #[test]
    fn resolve_name_from_env() {
        // SAFETY: This test is run in isolation and we're only modifying
        // an environment variable specific to this crate's tests.
        unsafe {
            std::env::set_var(EMERGENT_NAME_ENV, "from_env");
        }
        let name = resolve_name(None, "default");
        assert_eq!(name, "from_env");
        // SAFETY: Cleanup after test
        unsafe {
            std::env::remove_var(EMERGENT_NAME_ENV);
        }
    }

    #[test]
    fn resolve_name_explicit_overrides_env() {
        // SAFETY: This test is run in isolation and we're only modifying
        // an environment variable specific to this crate's tests.
        unsafe {
            std::env::set_var(EMERGENT_NAME_ENV, "from_env");
        }
        let name = resolve_name(Some("explicit"), "default");
        assert_eq!(name, "explicit");
        // SAFETY: Cleanup after test
        unsafe {
            std::env::remove_var(EMERGENT_NAME_ENV);
        }
    }

    #[test]
    fn helper_error_display() {
        let err = HelperError::ConnectionFailed {
            name: "test".to_string(),
            error: "socket not found".to_string(),
        };
        assert!(err.to_string().contains("test"));
        assert!(err.to_string().contains("socket not found"));

        let err = HelperError::UserFunction("user error".to_string());
        assert!(err.to_string().contains("user error"));

        let err = HelperError::SubscribeFailed("sub failed".to_string());
        assert!(err.to_string().contains("sub failed"));

        let err = HelperError::SignalHandlerFailed("signal error".to_string());
        assert!(err.to_string().contains("signal error"));
    }
}
