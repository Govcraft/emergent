//! Convenience functions for building Emergent primitives.
//!
//! These helpers eliminate the boilerplate code for connecting, handling signals,
//! and running the event loop. Developers only need to provide their business logic
//! as an async closure.
//!
//! Every example below is compiled as a doctest. They are marked `no_run`
//! because they need a live engine to execute, but a signature change that
//! breaks the documented closure shape now fails the build.
//!
//! # Examples
//!
//! ## Source with custom logic (interval-based timer)
//!
//! ```rust,no_run
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
//! ```rust,no_run
//! use emergent_client::helpers::run_source;
//! use emergent_client::EmergentMessage;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_source(Some("webhook"), |source, mut shutdown| async move {
//!         // Start HTTP server, publish on each request
//!         let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
//!             .await
//!             .map_err(|e| e.to_string())?;
//!
//!         loop {
//!             tokio::select! {
//!                 _ = shutdown.changed() => break,
//!                 result = listener.accept() => {
//!                     let (_stream, _addr) = result.map_err(|e| e.to_string())?;
//!                     // Parse request, publish message...
//!                     source.publish(EmergentMessage::new("webhook.received"))
//!                         .await
//!                         .map_err(|e| e.to_string())?;
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
//! ```rust,no_run
//! use emergent_client::helpers::run_source;
//! use emergent_client::EmergentMessage;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     run_source(Some("one_shot"), |source, _shutdown| async move {
//!         // Run once and exit
//!         source.publish(EmergentMessage::new("startup.complete"))
//!             .await
//!             .map_err(|e| e.to_string())?;
//!         Ok(())
//!     }).await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Handler with message transformation
//!
//! ```rust,no_run
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
//! ```rust,no_run
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
use acton_reactive::ipc::IpcPushNotification;
use std::future::Future;
use thiserror::Error;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

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

/// Why a Source's shutdown watch fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceStopReason {
    /// The engine asked for a graceful stop with SIGTERM.
    Signal,
    /// The engine's IPC connection reached EOF. A Source is told nothing else
    /// when the engine is SIGKILLed or aborts.
    EngineDisconnected,
}

/// Resolve as soon as either `signal` completes or `engine_push` closes.
///
/// Pushes that do arrive are discarded: a Source subscribes to nothing, so
/// anything delivered on that channel is a broadcast it never asked for and
/// only the channel closing carries meaning. Given no channel (one was already
/// taken elsewhere) this waits on the signal alone, which is the behaviour
/// Sources had before EOF was observable.
async fn source_stop_reason<S>(
    signal: S,
    engine_push: Option<mpsc::Receiver<IpcPushNotification>>,
) -> SourceStopReason
where
    S: Future<Output = ()>,
{
    let Some(mut push_rx) = engine_push else {
        signal.await;
        return SourceStopReason::Signal;
    };

    tokio::pin!(signal);

    loop {
        tokio::select! {
            () = &mut signal => return SourceStopReason::Signal,
            push = push_rx.recv() => {
                if push.is_none() {
                    return SourceStopReason::EngineDisconnected;
                }
            }
        }
    }
}

/// Run a Source with custom logic.
///
/// This function handles all the boilerplate for running a Source:
/// - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
/// - Connects to the Emergent engine
/// - Sets up SIGTERM signal handling for graceful shutdown
/// - Watches the engine connection and signals shutdown when it reaches EOF,
///   so a Source stops instead of publishing into a dead socket when the
///   engine is SIGKILLed or aborts
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
/// ```rust,no_run
/// use emergent_client::helpers::run_source;
/// use emergent_client::EmergentMessage;
/// use serde_json::json;
/// use std::time::Duration;
///
/// # async fn doc() -> Result<(), Box<dyn std::error::Error>> {
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
/// # Ok(())
/// # }
/// ```
///
/// # Example: One-shot source
///
/// ```rust,no_run
/// # use emergent_client::helpers::run_source;
/// # use emergent_client::EmergentMessage;
/// # async fn doc() -> Result<(), Box<dyn std::error::Error>> {
/// run_source(Some("init"), |source, _shutdown| async move {
///     source.publish(EmergentMessage::new("system.init"))
///         .await
///         .map_err(|e| e.to_string())?;
///     Ok(())
/// }).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_source<F, Fut>(name: Option<&str>, run_fn: F) -> HelperResult<()>
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
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

    // A Source has no subscription stream to end, so watch the IPC push
    // channel instead. It closes on socket EOF, which is the only notice an
    // engine that was SIGKILLed or aborted ever gives.
    let engine_push = source.take_engine_push_channel();
    let watcher_name = resolved_name.clone();

    // Spawn the shutdown watcher task
    let signal_task = tokio::spawn(async move {
        let reason = source_stop_reason(
            async move {
                sigterm.recv().await;
            },
            engine_push,
        )
        .await;

        match reason {
            SourceStopReason::Signal => {
                debug!(primitive.name = %watcher_name, "source shutting down (SIGTERM)");
            }
            SourceStopReason::EngineDisconnected => {
                warn!(
                    primitive.name = %watcher_name,
                    "source shutting down (engine connection closed)"
                );
            }
        }

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
/// * `process_fn` - Async function called for each message with `(msg, handler)`.
///   The handler is passed by value: it is a cheap clone of the connected
///   handler, sharing one IPC connection, so the returned future can own it
///   and publish across `.await` points.
///
/// # Returns
///
/// Returns `Ok(())` on graceful shutdown or an error if something fails.
///
/// # Example
///
/// ```rust,no_run
/// use emergent_client::helpers::run_handler;
/// use emergent_client::EmergentMessage;
/// use serde_json::json;
///
/// # async fn doc() -> Result<(), Box<dyn std::error::Error>> {
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
/// # Ok(())
/// # }
/// ```
pub async fn run_handler<F, Fut>(
    name: Option<&str>,
    subscriptions: &[&str],
    process_fn: F,
) -> HelperResult<()>
where
    F: Fn(EmergentMessage, EmergentHandler) -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), String>> + Send,
{
    let resolved_name = resolve_name(name, "handler");

    let mut handler = EmergentHandler::connect(&resolved_name)
        .await
        .map_err(|e| HelperError::ConnectionFailed {
            name: resolved_name.clone(),
            error: e.to_string(),
        })?;

    let mut stream = handler
        .subscribe(subscriptions)
        .await
        .map_err(|e| HelperError::SubscribeFailed(e.to_string()))?;

    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

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
                        if let Err(e) = process_fn(msg, handler.clone()).await {
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
/// ```rust,no_run
/// use emergent_client::helpers::run_sink;
///
/// # async fn doc() -> Result<(), Box<dyn std::error::Error>> {
/// run_sink(
///     Some("my_sink"),
///     &["timer.processed"],
///     |msg| async move {
///         println!("Received: {:?}", msg.payload());
///         Ok(())
///     }
/// ).await?;
/// # Ok(())
/// # }
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

    let mut sink =
        EmergentSink::connect(&resolved_name)
            .await
            .map_err(|e| HelperError::ConnectionFailed {
                name: resolved_name.clone(),
                error: e.to_string(),
            })?;

    let mut stream = sink
        .subscribe(subscriptions)
        .await
        .map_err(|e| HelperError::SubscribeFailed(e.to_string()))?;

    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| HelperError::SignalHandlerFailed(e.to_string()))?;

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

    fn push(message_type: &str) -> IpcPushNotification {
        IpcPushNotification::new(message_type, None, serde_json::Value::Null)
    }

    /// A Source that outlives its engine must learn about it somehow, and the
    /// push channel closing is the only notice it gets.
    #[tokio::test]
    async fn engine_eof_stops_a_source() {
        let (tx, rx) = mpsc::channel(4);
        drop(tx);

        let reason = source_stop_reason(std::future::pending::<()>(), Some(rx)).await;

        assert_eq!(reason, SourceStopReason::EngineDisconnected);
    }

    /// SIGTERM still wins while the engine connection is healthy.
    #[tokio::test]
    async fn sigterm_stops_a_source_with_a_live_engine() {
        let (tx, rx) = mpsc::channel(4);

        let reason = source_stop_reason(std::future::ready(()), Some(rx)).await;

        assert_eq!(reason, SourceStopReason::Signal);
        drop(tx);
    }

    /// Anything actually delivered on the channel is noise to a Source, and
    /// must not be mistaken for the engine going away.
    #[tokio::test]
    async fn a_delivered_push_does_not_stop_a_source() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(push("timer.tick")).await.ok();
        tx.send(push("system.started.timer")).await.ok();

        let watch = tokio::spawn(source_stop_reason(std::future::pending::<()>(), Some(rx)));

        // Still waiting: the pushes were consumed, not treated as a close.
        let early = tokio::time::timeout(std::time::Duration::from_millis(50), watch).await;
        assert!(early.is_err(), "a delivered push must not stop the source");
    }

    /// With no channel to watch, a Source behaves exactly as it did before.
    #[tokio::test]
    async fn without_a_push_channel_only_the_signal_stops_a_source() {
        let reason = source_stop_reason(std::future::ready(()), None).await;

        assert_eq!(reason, SourceStopReason::Signal);
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
