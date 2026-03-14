//! Connection types for Emergent primitives.
//!
//! This module provides the three primitive types that connect to the Emergent engine:
//! - [`EmergentSource`] - publish only
//! - [`EmergentHandler`] - subscribe and publish
//! - [`EmergentSink`] - subscribe only

use crate::error::ClientError;
use crate::message::EmergentMessage;
use crate::stream::MessageStream;
use crate::subscribe::IntoSubscription;
use crate::types::{CorrelationId, PrimitiveName};
use crate::{DiscoveryInfo, PrimitiveInfo, Result};

use tracing::{debug, error, info, warn};

use acton_reactive::ipc::protocol::{
    Format, MAX_FRAME_SIZE, MSG_TYPE_DISCOVER, MSG_TYPE_PUSH, MSG_TYPE_REQUEST, MSG_TYPE_RESPONSE,
    MSG_TYPE_SUBSCRIBE, MSG_TYPE_UNSUBSCRIBE, read_frame, write_frame,
};
use acton_reactive::ipc::{
    IpcConfig, IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcPushNotification,
    IpcSubscribeRequest, IpcSubscriptionResponse, IpcUnsubscribeRequest, socket_exists,
    socket_is_alive,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

/// Default timeout for connection operations.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// IPC wrapper message for EmergentMessage.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct IpcEmergentMessage {
    inner: EmergentMessage,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolve the socket path from environment or config.
fn resolve_socket_path(_name: &str) -> Result<PathBuf> {
    // First check EMERGENT_SOCKET environment variable
    if let Ok(path) = std::env::var("EMERGENT_SOCKET") {
        return Ok(PathBuf::from(path));
    }

    // Then check EMERGENT_NAME for the client name (set by engine when spawning)
    // This allows the engine to pass the socket path

    // Fall back to XDG-compliant default using IpcConfig
    let mut config = IpcConfig::load();
    config.socket.app_name = Some("emergent".to_string());
    Ok(config.socket_path())
}

/// Initialize a default tracing subscriber if one hasn't been set.
///
/// Logs to `~/.local/share/emergent/<name>/primitive.log` by default.
/// Set `EMERGENT_LOG=stderr` to log to stderr instead (for debugging).
/// No-op if the primitive already installed a subscriber.
fn init_tracing(name: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("EMERGENT_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Check if user explicitly wants stderr output
    let wants_stderr = std::env::var("EMERGENT_LOG")
        .map(|v| v.eq_ignore_ascii_case("stderr"))
        .unwrap_or(false);

    if wants_stderr {
        let stderr_filter = EnvFilter::new("info");
        let _ = tracing_subscriber::fmt()
            .with_env_filter(stderr_filter)
            .try_init();
    } else {
        // Log to file in XDG data directory, keyed by primitive name
        let log_dir =
            directories::ProjectDirs::from("ai", "govcraft", "emergent")
                .map(|dirs| dirs.data_dir().join(name))
                .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&log_dir);

        if let Ok(log_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("primitive.log"))
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::sync::Mutex::new(log_file))
                .with_ansi(false)
                .try_init();
        } else {
            // If we can't open the file, fall back to silent
            let _ = tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::new("off"))
                .try_init();
        }
    }
}

/// Connect to the engine socket with health checks.
async fn connect_to_engine(name: &str) -> Result<UnixStream> {
    init_tracing(name);
    let socket_path = resolve_socket_path(name)?;
    debug!(path = %socket_path.display(), "resolved socket path");

    info!(primitive.name = %name, path = %socket_path.display(), "connecting to engine");

    if !socket_exists(&socket_path) {
        error!(path = %socket_path.display(), "engine socket not found");
        return Err(ClientError::SocketNotFound(
            socket_path.display().to_string(),
        ));
    }

    if !socket_is_alive(&socket_path).await {
        error!(path = %socket_path.display(), "engine socket not responding");
        return Err(ClientError::ConnectionFailed(
            "Engine socket exists but is not responding".to_string(),
        ));
    }

    UnixStream::connect(&socket_path).await.map_err(|e| {
        error!(error = %e, "failed to connect to engine");
        ClientError::ConnectionFailed(e.to_string())
    })
}

/// Spawn a background task that reads and discards all incoming frames.
///
/// Prevents the OS socket buffer from filling when the server sends
/// `MSG_TYPE_RESPONSE` back for each `MSG_TYPE_REQUEST` publish. Without this
/// drain, the server's writer blocks, its read loop stalls, and the client's
/// `write_frame` blocks indefinitely after ~100 seconds of 1-second publishing.
fn spawn_drain_task(mut reader: OwnedReadHalf, name: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while read_frame(&mut reader, MAX_FRAME_SIZE).await.is_ok() {}
        debug!(primitive.name = %name, "response drain stopped");
    })
}

/// Send a discover request and get available message types.
async fn discover_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
) -> Result<DiscoveryInfo> {
    debug!("sending discovery request");

    let request = IpcDiscoverRequest::new();
    let payload = rmp_serde::to_vec_named(&request)?;

    write_frame(writer, MSG_TYPE_DISCOVER, Format::MessagePack, &payload)
        .await
        .map_err(ClientError::from)?;

    let (msg_type, _, payload) = timeout(DEFAULT_TIMEOUT, read_frame(reader, MAX_FRAME_SIZE))
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(ClientError::from)?;

    if msg_type != MSG_TYPE_RESPONSE {
        return Err(ClientError::ProtocolError(format!(
            "Expected RESPONSE, got message type {msg_type}"
        )));
    }

    let response: IpcDiscoverResponse = rmp_serde::from_slice(&payload)?;

    if !response.success {
        return Err(ClientError::DiscoveryFailed(
            response
                .error
                .unwrap_or_else(|| "Unknown error".to_string()),
        ));
    }

    let info = DiscoveryInfo {
        message_types: response.message_types.unwrap_or_default(),
        primitives: response
            .actors
            .unwrap_or_default()
            .into_iter()
            .map(|a| PrimitiveInfo {
                name: a.name,
                kind: "unknown".to_string(),
            })
            .collect(),
    };

    debug!(
        message_type_count = info.message_types.len(),
        primitive_count = info.primitives.len(),
        "discovery complete"
    );

    Ok(info)
}

/// Subscribe to message types.
async fn subscribe_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    types: &[&str],
) -> Result<Vec<String>> {
    info!(types = ?types, "subscribing to message types");

    let request = IpcSubscribeRequest::new(types.iter().map(|s| (*s).to_string()).collect());
    let payload = rmp_serde::to_vec_named(&request)?;

    write_frame(writer, MSG_TYPE_SUBSCRIBE, Format::MessagePack, &payload)
        .await
        .map_err(ClientError::from)?;

    let (msg_type, _, payload) = timeout(DEFAULT_TIMEOUT, read_frame(reader, MAX_FRAME_SIZE))
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(ClientError::from)?;

    if msg_type != MSG_TYPE_RESPONSE {
        return Err(ClientError::ProtocolError(format!(
            "Expected RESPONSE, got message type {msg_type}"
        )));
    }

    let response: IpcSubscriptionResponse = rmp_serde::from_slice(&payload)?;

    if !response.success {
        return Err(ClientError::SubscriptionFailed(
            response
                .error
                .unwrap_or_else(|| "Unknown error".to_string()),
        ));
    }

    info!(subscribed_count = response.subscribed_types.len(), "subscribed to message types");

    Ok(response.subscribed_types)
}

/// Unsubscribe from message types.
async fn unsubscribe_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    types: &[&str],
) -> Result<()> {
    debug!(types = ?types, "unsubscribing from message types");

    let request = if types.is_empty() {
        IpcUnsubscribeRequest::unsubscribe_all()
    } else {
        IpcUnsubscribeRequest::new(types.iter().map(|s| (*s).to_string()).collect())
    };
    let payload = rmp_serde::to_vec_named(&request)?;

    write_frame(writer, MSG_TYPE_UNSUBSCRIBE, Format::MessagePack, &payload)
        .await
        .map_err(ClientError::from)?;

    let (msg_type, _, payload) = timeout(DEFAULT_TIMEOUT, read_frame(reader, MAX_FRAME_SIZE))
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(ClientError::from)?;

    if msg_type != MSG_TYPE_RESPONSE {
        return Err(ClientError::ProtocolError(format!(
            "Expected RESPONSE, got message type {msg_type}"
        )));
    }

    let response: IpcSubscriptionResponse = rmp_serde::from_slice(&payload)?;

    if !response.success {
        let err = ClientError::SubscriptionFailed(
            response
                .error
                .unwrap_or_else(|| "Unknown error".to_string()),
        );
        warn!(error = %err, "unsubscribe failed");
        return Err(err);
    }

    Ok(())
}

/// Publish a message (fire-and-forget).
async fn publish_impl(writer: &mut OwnedWriteHalf, message: EmergentMessage) -> Result<()> {
    debug!(message_type = %message.message_type, message_id = %message.id, "publishing message");

    let ipc_message = IpcEmergentMessage { inner: message };
    let envelope = IpcEnvelope::new(
        "message_broker",
        "EmergentMessage",
        serde_json::to_value(&ipc_message)?,
    );

    let payload = rmp_serde::to_vec_named(&envelope)?;
    write_frame(writer, MSG_TYPE_REQUEST, Format::MessagePack, &payload)
        .await
        .map_err(ClientError::from)?;

    Ok(())
}

/// Response from GetSubscriptions request.
#[derive(Debug, Deserialize)]
struct SubscriptionsResponse {
    subscribes: Vec<String>,
}

/// Information about a primitive in the topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyPrimitive {
    /// Unique name of the primitive.
    pub name: String,
    /// Kind of primitive (source, handler, sink).
    pub kind: String,
    /// Current lifecycle state.
    pub state: String,
    /// Message types this primitive publishes.
    pub publishes: Vec<String>,
    /// Message types this primitive subscribes to.
    pub subscribes: Vec<String>,
    /// Process ID if running.
    pub pid: Option<u32>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Response from GetTopology request.
#[derive(Debug, Deserialize)]
struct TopologyResponse {
    primitives: Vec<TopologyPrimitive>,
}

/// Current topology state (all primitives).
#[derive(Debug, Clone)]
pub struct TopologyState {
    /// All primitives in the system.
    pub primitives: Vec<TopologyPrimitive>,
}

/// Get configured subscriptions from the engine via pub/sub.
///
/// Uses the `system.request.subscriptions` / `system.response.subscriptions` pattern.
async fn get_my_subscriptions_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    name: &str,
) -> Result<Vec<String>> {
    debug!("querying configured subscriptions");

    // Generate correlation ID for matching response
    let correlation_id = CorrelationId::new();

    // Subscribe to response type first
    subscribe_impl(reader, writer, &["system.response.subscriptions"]).await?;

    // Create and publish request message
    let request = EmergentMessage::new("system.request.subscriptions")
        .with_source(name)
        .with_correlation_id(correlation_id.clone())
        .with_payload(json!({ "name": name }));

    publish_impl(writer, request).await?;

    // Wait for response with matching correlation_id
    let subs: Result<Vec<String>> = timeout(DEFAULT_TIMEOUT, async {
        loop {
            let (msg_type, format, payload) = read_frame(reader, MAX_FRAME_SIZE)
                .await
                .map_err(ClientError::from)?;

            if msg_type == MSG_TYPE_PUSH {
                let notification: IpcPushNotification = format.deserialize(&payload)?;
                if notification.message_type == "system.response.subscriptions" {
                    // Parse the EmergentMessage from payload
                    let msg: EmergentMessage = serde_json::from_value(notification.payload)?;
                    // Check correlation_id matches
                    if msg.correlation_id.as_ref().map(|c| c.to_string())
                        == Some(correlation_id.to_string())
                    {
                        // Extract subscribes from payload
                        let subs_response: SubscriptionsResponse =
                            serde_json::from_value(msg.payload)?;
                        return Ok(subs_response.subscribes);
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| ClientError::Timeout)?;

    let subs = subs?;
    info!(types = ?subs, "received configured subscriptions");
    Ok(subs)
}

/// Get current topology from the engine via pub/sub.
///
/// Uses the `system.request.topology` / `system.response.topology` pattern.
async fn get_topology_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    name: &str,
) -> Result<TopologyState> {
    debug!("querying topology");

    // Generate correlation ID for matching response
    let correlation_id = CorrelationId::new();

    // Subscribe to response type first
    subscribe_impl(reader, writer, &["system.response.topology"]).await?;

    // Create and publish request message
    let request = EmergentMessage::new("system.request.topology")
        .with_source(name)
        .with_correlation_id(correlation_id.clone())
        .with_payload(json!({}));

    publish_impl(writer, request).await?;

    // Wait for response with matching correlation_id
    let state: Result<TopologyState> = timeout(DEFAULT_TIMEOUT, async {
        loop {
            let (msg_type, format, payload) = read_frame(reader, MAX_FRAME_SIZE)
                .await
                .map_err(ClientError::from)?;

            if msg_type == MSG_TYPE_PUSH {
                let notification: IpcPushNotification = format.deserialize(&payload)?;
                if notification.message_type == "system.response.topology" {
                    // Parse the EmergentMessage from payload
                    let msg: EmergentMessage = serde_json::from_value(notification.payload)?;
                    // Check correlation_id matches
                    if msg.correlation_id.as_ref().map(|c| c.to_string())
                        == Some(correlation_id.to_string())
                    {
                        // Extract primitives from payload
                        let topo_response: TopologyResponse = serde_json::from_value(msg.payload)?;
                        return Ok(TopologyState {
                            primitives: topo_response.primitives,
                        });
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| ClientError::Timeout)?;

    let state = state?;
    debug!(primitive_count = state.primitives.len(), "received topology");
    Ok(state)
}

// ============================================================================
// EmergentSource - Publish Only
// ============================================================================

/// A Source primitive that publishes messages to the Emergent engine.
///
/// Sources are the ingress point for data entering the workflow. They can only
/// publish messages (fire-and-forget) and cannot subscribe to receive messages.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::{EmergentSource, EmergentMessage};
/// use serde_json::json;
///
/// let source = EmergentSource::connect("my_source").await?;
///
/// loop {
///     let message = EmergentMessage::new("sensor.reading")
///         .with_payload(json!({"temperature": 72.5}));
///     source.publish(message).await?;
///     tokio::time::sleep(Duration::from_secs(1)).await;
/// }
/// ```
pub struct EmergentSource {
    /// Name of this source.
    name: String,
    /// Writer half of the socket connection.
    writer: Arc<Mutex<OwnedWriteHalf>>,
    /// Handle to abort the background response-drain task on shutdown.
    drain_abort: tokio::task::AbortHandle,
}

impl EmergentSource {
    /// Connect to the Emergent engine as a Source.
    ///
    /// The `name` parameter identifies this source in logs and tracing.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails or the engine is not running.
    pub async fn connect(name: &str) -> Result<Self> {
        let stream = connect_to_engine(name).await?;
        let (reader, writer) = stream.into_split();

        let drain_handle = spawn_drain_task(reader, name.to_string());

        info!(primitive.name = %name, primitive.kind = "source", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            drain_abort: drain_handle.abort_handle(),
        })
    }

    /// Publish a message to the engine (fire-and-forget).
    ///
    /// The message will be routed to any Handlers or Sinks subscribed to its type.
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent.
    pub async fn publish(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let mut writer = self.writer.lock().await;
        publish_impl(&mut writer, message).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "failed to publish message");
            e
        })
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        discover_impl(&mut reader, &mut writer).await
    }

    /// Get the name of this source.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gracefully disconnect from the engine.
    ///
    /// This sends an unsubscribe-all message to signal the server that this client
    /// is disconnecting, allowing for clean connection teardown.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");

        let mut writer = self.writer.lock().await;

        // Send unsubscribe-all as a "goodbye" signal (fire-and-forget).
        // The drain task consumes any response.
        let request = IpcUnsubscribeRequest::unsubscribe_all();
        let payload = rmp_serde::to_vec_named(&request)?;
        let _ = write_frame(
            &mut *writer,
            MSG_TYPE_UNSUBSCRIBE,
            Format::MessagePack,
            &payload,
        )
        .await;

        // Shutdown writer — causes the drain task to see EOF and exit
        use tokio::io::AsyncWriteExt;
        let _ = writer.shutdown().await;

        // Stop the drain task
        self.drain_abort.abort();

        info!(primitive.name = %self.name, "disconnected from engine");

        Ok(())
    }
}

impl Drop for EmergentSource {
    fn drop(&mut self) {
        self.drain_abort.abort();
    }
}

// ============================================================================
// EmergentHandler - Subscribe and Publish
// ============================================================================

/// A Handler primitive that subscribes to and publishes messages.
///
/// Handlers are the transformation layer in the workflow. They receive messages,
/// process them, and publish new messages based on the results.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::{EmergentHandler, EmergentMessage};
/// use serde_json::json;
///
/// let handler = EmergentHandler::connect("my_handler").await?;
/// let mut stream = handler.subscribe(&["sensor.reading"]).await?;
///
/// while let Some(msg) = stream.next().await {
///     // Process the message
///     let temp: f64 = msg.payload["temperature"].as_f64().unwrap_or(0.0);
///
///     if temp > 80.0 {
///         let alert = EmergentMessage::new("alert.high_temp")
///             .with_causation_id(msg.id())
///             .with_payload(json!({"temperature": temp}));
///         handler.publish(alert).await?;
///     }
/// }
/// ```
pub struct EmergentHandler {
    /// Name of this handler.
    name: String,
    /// Writer half of the socket connection.
    writer: Arc<Mutex<OwnedWriteHalf>>,
    /// Currently subscribed message types.
    subscribed_types: Arc<Mutex<Vec<String>>>,
    /// Handle to abort the background response-drain task on shutdown.
    drain_abort: tokio::task::AbortHandle,
}

impl EmergentHandler {
    /// Connect to the Emergent engine as a Handler.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(name: &str) -> Result<Self> {
        let stream = connect_to_engine(name).await?;
        let (reader, writer) = stream.into_split();

        let drain_handle = spawn_drain_task(reader, name.to_string());

        info!(primitive.name = %name, primitive.kind = "handler", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            subscribed_types: Arc::new(Mutex::new(Vec::new())),
            drain_abort: drain_handle.abort_handle(),
        })
    }

    /// Subscribe to message types and return a stream of incoming messages.
    ///
    /// The SDK automatically handles `system.shutdown` messages - when the engine
    /// signals shutdown for handlers, the stream will close gracefully.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Single topic
    /// let stream = handler.subscribe("timer.tick").await?;
    ///
    /// // Multiple topics with array
    /// let stream = handler.subscribe(["timer.tick", "timer.filtered"]).await?;
    ///
    /// // From a Vec
    /// let topics = vec!["timer.tick".to_string()];
    /// let stream = handler.subscribe(topics).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription fails.
    pub async fn subscribe(&self, types: impl IntoSubscription) -> Result<MessageStream> {
        let topics = types.into_topics();

        // Create a new connection for receiving (subscriptions need dedicated reader)
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        // Add system.shutdown to subscriptions (SDK handles it internally)
        let mut all_types: Vec<&str> = topics.iter().map(String::as_str).collect();
        if !all_types.contains(&"system.shutdown") {
            all_types.push("system.shutdown");
        }

        // Subscribe
        let subscribed = subscribe_impl(&mut reader, &mut writer, &all_types).await?;

        // Update tracked subscriptions (excluding internal system.shutdown)
        {
            let mut subs = self.subscribed_types.lock().await;
            *subs = subscribed
                .into_iter()
                .filter(|s| s != "system.shutdown")
                .collect();
        }

        // Create channel for message stream
        let (tx, rx) = mpsc::channel(256);
        let log_name = self.name.clone();

        // Spawn task to read push notifications
        // IMPORTANT: We must keep the writer alive even though we don't use it.
        // Dropping it would half-close the socket and cause the server to close the connection.
        tokio::spawn(async move {
            // Keep writer alive by moving it into the task (but don't use it)
            let _writer = writer;
            debug!(primitive.name = %log_name, "read loop started");

            loop {
                match read_frame(&mut reader, MAX_FRAME_SIZE).await {
                    Ok((msg_type, format, payload)) => {
                        if msg_type == MSG_TYPE_PUSH {
                            // Deserialize push notification using the format from the frame header
                            match format.deserialize::<IpcPushNotification>(&payload) {
                                Ok(notification) => {
                                    // Check for shutdown signal
                                    if notification.message_type == "system.shutdown" {
                                        let shutdown_kind = notification
                                            .payload
                                            .get("kind")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown");
                                        info!(
                                            primitive.name = %log_name,
                                            shutdown_kind = %shutdown_kind,
                                            "received shutdown signal"
                                        );
                                        if shutdown_kind == "handler" {
                                            info!(
                                                primitive.name = %log_name,
                                                "shutting down (engine requested)"
                                            );
                                            break;
                                        }
                                        debug!(
                                            primitive.name = %log_name,
                                            "ignoring shutdown for different primitive kind"
                                        );
                                        continue; // Don't forward system.shutdown to user
                                    }

                                    // Try to extract EmergentMessage from payload
                                    // The broker sends EmergentMessage directly as the payload
                                    if let Ok(msg) = serde_json::from_value::<EmergentMessage>(
                                        notification.payload.clone(),
                                    ) {
                                        debug!(
                                            primitive.name = %log_name,
                                            message_type = %msg.message_type,
                                            message_id = %msg.id,
                                            "received message"
                                        );
                                        if tx.send(msg).await.is_err() {
                                            warn!(
                                                primitive.name = %log_name,
                                                "message stream send failed, receiver dropped"
                                            );
                                            break;
                                        }
                                    } else {
                                        // Fallback: create EmergentMessage from push notification fields
                                        let msg = EmergentMessage::new(&notification.message_type)
                                            .with_source(
                                                notification
                                                    .source_actor
                                                    .as_deref()
                                                    .unwrap_or("unknown"),
                                            )
                                            .with_payload(notification.payload);
                                        debug!(
                                            primitive.name = %log_name,
                                            message_type = %msg.message_type,
                                            message_id = %msg.id,
                                            "received message"
                                        );
                                        if tx.send(msg).await.is_err() {
                                            warn!(
                                                primitive.name = %log_name,
                                                "message stream send failed, receiver dropped"
                                            );
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        primitive.name = %log_name,
                                        error = %e,
                                        "failed to parse push notification"
                                    );
                                }
                            }
                        }
                        // Ignore non-PUSH frames (heartbeats, etc.)
                    }
                    Err(e) => {
                        info!(
                            primitive.name = %log_name,
                            error = %e,
                            "connection closed"
                        );
                        break;
                    }
                }
            }
        });

        Ok(MessageStream::new(rx))
    }

    /// Unsubscribe from message types.
    ///
    /// # Errors
    ///
    /// Returns an error if the unsubscription fails.
    pub async fn unsubscribe(&self, types: &[&str]) -> Result<()> {
        // Create connection for unsubscribe request
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        unsubscribe_impl(&mut reader, &mut writer, types).await?;

        // Update tracked subscriptions
        {
            let mut subs = self.subscribed_types.lock().await;
            for t in types {
                subs.retain(|s| s != *t);
            }
        }

        Ok(())
    }

    /// Publish a message to the engine (fire-and-forget).
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent.
    pub async fn publish(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let mut writer = self.writer.lock().await;
        publish_impl(&mut writer, message).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "failed to publish message");
            e
        })
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        discover_impl(&mut reader, &mut writer).await
    }

    /// Get the configured subscription types for this primitive.
    ///
    /// Queries the engine's config service to get the message types
    /// this handler should subscribe to based on the engine configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_my_subscriptions(&self) -> Result<Vec<String>> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        get_my_subscriptions_impl(&mut reader, &mut writer, &self.name).await
    }

    /// Get the name of this handler.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get currently subscribed message types.
    pub async fn subscribed_types(&self) -> Vec<String> {
        self.subscribed_types.lock().await.clone()
    }

    /// Gracefully disconnect from the engine.
    ///
    /// This unsubscribes from all message types and cleanly closes the connection,
    /// allowing the server to see a normal EOF instead of a connection reset error.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");
        self.drain_abort.abort();
        // Unsubscribe from all message types
        self.unsubscribe(&[]).await?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
    }
}

impl Drop for EmergentHandler {
    fn drop(&mut self) {
        self.drain_abort.abort();
    }
}

// ============================================================================
// EmergentSink - Subscribe Only
// ============================================================================

/// A Sink primitive that subscribes to messages from the Emergent engine.
///
/// Sinks are the egress point for data leaving the workflow. They receive messages
/// but cannot publish new messages to the bus.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::EmergentSink;
///
/// let sink = EmergentSink::connect("my_sink").await?;
/// let mut stream = sink.subscribe(&["alert.high_temp"]).await?;
///
/// while let Some(msg) = stream.next().await {
///     println!("[ALERT] Temperature: {}", msg.payload["temperature"]);
///     // Log to file, send notification, etc.
/// }
/// ```
pub struct EmergentSink {
    /// Name of this sink.
    name: String,
    /// Currently subscribed message types.
    subscribed_types: Arc<Mutex<Vec<String>>>,
}

impl EmergentSink {
    /// Connect to the Emergent engine as a Sink.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(name: &str) -> Result<Self> {
        // Verify connection is possible (but don't hold it)
        let _ = connect_to_engine(name).await?;

        info!(primitive.name = %name, primitive.kind = "sink", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            subscribed_types: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Convenience method that connects, gets configured subscriptions, and returns a stream.
    ///
    /// This is a one-liner for the common pattern of:
    /// 1. Connect to the engine
    /// 2. Query configured subscriptions from the engine's config
    /// 3. Subscribe to those topics
    /// 4. Return the message stream
    ///
    /// The `types` parameter is for API consistency but is ignored - the engine's
    /// configuration is the source of truth for what this sink should subscribe to.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use futures::StreamExt;
    ///
    /// let mut stream = EmergentSink::messages("console", ["timer.tick"]).await?;
    /// while let Some(msg) = stream.next().await {
    ///     println!("{}", msg.payload);
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if connection or subscription fails.
    pub async fn messages(
        name: impl Into<String>,
        _types: impl IntoSubscription,
    ) -> Result<MessageStream> {
        let name = name.into();
        let sink = Self::connect(&name).await?;
        let topics = sink.get_my_subscriptions().await?;
        sink.subscribe(topics).await
    }

    /// Subscribe to message types and return a stream of incoming messages.
    ///
    /// The SDK automatically handles `system.shutdown` messages - when the engine
    /// signals shutdown for sinks, the stream will close gracefully.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Single topic
    /// let stream = sink.subscribe("timer.tick").await?;
    ///
    /// // Multiple topics with array
    /// let stream = sink.subscribe(["timer.tick", "timer.filtered"]).await?;
    ///
    /// // From a Vec
    /// let topics = vec!["timer.tick".to_string()];
    /// let stream = sink.subscribe(topics).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription fails.
    pub async fn subscribe(&self, types: impl IntoSubscription) -> Result<MessageStream> {
        let topics = types.into_topics();

        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        // Add system.shutdown to subscriptions (SDK handles it internally)
        let mut all_types: Vec<&str> = topics.iter().map(String::as_str).collect();
        if !all_types.contains(&"system.shutdown") {
            all_types.push("system.shutdown");
        }

        // Subscribe
        let subscribed = subscribe_impl(&mut reader, &mut writer, &all_types).await?;

        // Update tracked subscriptions (excluding internal system.shutdown)
        {
            let mut subs = self.subscribed_types.lock().await;
            *subs = subscribed
                .into_iter()
                .filter(|s| s != "system.shutdown")
                .collect();
        }

        // Create channel for message stream
        let (tx, rx) = mpsc::channel(256);
        let log_name = self.name.clone();

        // Spawn task to read push notifications
        // IMPORTANT: We must keep the writer alive even though we don't use it.
        // Dropping it would half-close the socket and cause the server to close the connection.
        tokio::spawn(async move {
            // Keep writer alive by moving it into the task (but don't use it)
            let _writer = writer;
            debug!(primitive.name = %log_name, "read loop started");

            loop {
                match read_frame(&mut reader, MAX_FRAME_SIZE).await {
                    Ok((msg_type, format, payload)) => {
                        if msg_type == MSG_TYPE_PUSH {
                            // Deserialize push notification using the format from the frame header
                            match format.deserialize::<IpcPushNotification>(&payload) {
                                Ok(notification) => {
                                    // Check for shutdown signal
                                    if notification.message_type == "system.shutdown" {
                                        let shutdown_kind = notification
                                            .payload
                                            .get("kind")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown");
                                        info!(
                                            primitive.name = %log_name,
                                            shutdown_kind = %shutdown_kind,
                                            "received shutdown signal"
                                        );
                                        if shutdown_kind == "sink" {
                                            info!(
                                                primitive.name = %log_name,
                                                "shutting down (engine requested)"
                                            );
                                            break;
                                        }
                                        debug!(
                                            primitive.name = %log_name,
                                            "ignoring shutdown for different primitive kind"
                                        );
                                        continue; // Don't forward system.shutdown to user
                                    }

                                    // Try to extract EmergentMessage from payload
                                    // The broker sends EmergentMessage directly as the payload
                                    if let Ok(msg) = serde_json::from_value::<EmergentMessage>(
                                        notification.payload.clone(),
                                    ) {
                                        debug!(
                                            primitive.name = %log_name,
                                            message_type = %msg.message_type,
                                            message_id = %msg.id,
                                            "received message"
                                        );
                                        if tx.send(msg).await.is_err() {
                                            warn!(
                                                primitive.name = %log_name,
                                                "message stream send failed, receiver dropped"
                                            );
                                            break;
                                        }
                                    } else {
                                        // Fallback: create EmergentMessage from push notification fields
                                        let msg = EmergentMessage::new(&notification.message_type)
                                            .with_source(
                                                notification
                                                    .source_actor
                                                    .as_deref()
                                                    .unwrap_or("unknown"),
                                            )
                                            .with_payload(notification.payload);
                                        debug!(
                                            primitive.name = %log_name,
                                            message_type = %msg.message_type,
                                            message_id = %msg.id,
                                            "received message"
                                        );
                                        if tx.send(msg).await.is_err() {
                                            warn!(
                                                primitive.name = %log_name,
                                                "message stream send failed, receiver dropped"
                                            );
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        primitive.name = %log_name,
                                        error = %e,
                                        "failed to parse push notification"
                                    );
                                }
                            }
                        }
                        // Ignore non-PUSH frames (heartbeats, etc.)
                    }
                    Err(e) => {
                        info!(
                            primitive.name = %log_name,
                            error = %e,
                            "connection closed"
                        );
                        break;
                    }
                }
            }
        });

        Ok(MessageStream::new(rx))
    }

    /// Unsubscribe from message types.
    ///
    /// # Errors
    ///
    /// Returns an error if the unsubscription fails.
    pub async fn unsubscribe(&self, types: &[&str]) -> Result<()> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        unsubscribe_impl(&mut reader, &mut writer, types).await?;

        {
            let mut subs = self.subscribed_types.lock().await;
            for t in types {
                subs.retain(|s| s != *t);
            }
        }

        Ok(())
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        discover_impl(&mut reader, &mut writer).await
    }

    /// Get the configured subscription types for this primitive.
    ///
    /// Queries the engine's config service to get the message types
    /// this sink should subscribe to based on the engine configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_my_subscriptions(&self) -> Result<Vec<String>> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        get_my_subscriptions_impl(&mut reader, &mut writer, &self.name).await
    }

    /// Get the current topology (all primitives and their state).
    ///
    /// Queries the engine to get the current state of all registered
    /// primitives, including their publish/subscribe configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_topology(&self) -> Result<TopologyState> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();
        get_topology_impl(&mut reader, &mut writer, &self.name).await
    }

    /// Get the name of this sink.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get currently subscribed message types.
    pub async fn subscribed_types(&self) -> Vec<String> {
        self.subscribed_types.lock().await.clone()
    }

    /// Gracefully disconnect from the engine.
    ///
    /// This unsubscribes from all message types and cleanly closes the connection,
    /// allowing the server to see a normal EOF instead of a connection reset error.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");
        // Unsubscribe from all message types
        self.unsubscribe(&[]).await?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
    }
}
