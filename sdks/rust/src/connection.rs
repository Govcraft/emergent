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

use acton_reactive::ipc::protocol::Format;
use acton_reactive::ipc::{
    IpcClient, IpcClientConfig, IpcConfig, IpcEnvelope, IpcPushNotification, socket_exists,
    socket_is_alive,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// IPC wrapper message for `EmergentMessage`.
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
        let log_dir = directories::ProjectDirs::from("ai", "govcraft", "emergent")
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

/// Connect to the engine socket with health checks, returning an `IpcClient`.
///
/// If `socket_override` is `Some`, uses that path directly. Otherwise resolves
/// the socket path from `EMERGENT_SOCKET` env var or XDG default.
async fn connect_to_engine(
    name: &str,
    socket_override: Option<&std::path::Path>,
) -> Result<IpcClient> {
    init_tracing(name);
    let socket_path = match socket_override {
        Some(path) => path.to_path_buf(),
        None => resolve_socket_path(name)?,
    };
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

    let config = IpcClientConfig {
        format: Format::MessagePack,
        ..IpcClientConfig::default()
    };

    IpcClient::connect_with_config(&socket_path, config)
        .await
        .map_err(|e| {
            error!(error = %e, "failed to connect to engine");
            ClientError::ConnectionFailed(e.to_string())
        })
}

/// Build an `IpcEnvelope` for publishing an `EmergentMessage` (fire-and-forget).
fn build_publish_envelope(message: EmergentMessage) -> Result<IpcEnvelope> {
    let ipc_message = IpcEmergentMessage { inner: message };
    let payload = serde_json::to_value(&ipc_message)?;
    Ok(IpcEnvelope::new(
        "message_broker",
        "EmergentMessage",
        payload,
    ))
}

/// Bridge push notifications from an `IpcClient` to a `MessageStream`.
///
/// Handles `system.shutdown` detection and `EmergentMessage` extraction.
/// This replaces the duplicated ~100-line read loops that were previously
/// copy-pasted between Handler and Sink.
async fn push_to_message_stream(
    mut push_rx: mpsc::Receiver<IpcPushNotification>,
    tx: mpsc::Sender<EmergentMessage>,
    name: String,
    shutdown_kind: &str,
) {
    debug!(primitive.name = %name, "push bridge started");

    while let Some(notification) = push_rx.recv().await {
        // Check for shutdown signal
        if notification.message_type == "system.shutdown" {
            let kind = notification
                .payload
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            info!(
                primitive.name = %name,
                shutdown_kind = %kind,
                "received shutdown signal"
            );
            if kind == shutdown_kind {
                info!(
                    primitive.name = %name,
                    "shutting down (engine requested)"
                );
                break;
            }
            debug!(
                primitive.name = %name,
                "ignoring shutdown for different primitive kind"
            );
            continue; // Don't forward system.shutdown to user
        }

        // Try to extract EmergentMessage from payload
        let msg = if let Ok(msg) =
            serde_json::from_value::<EmergentMessage>(notification.payload.clone())
        {
            msg
        } else {
            // Fallback: create EmergentMessage from push notification fields
            EmergentMessage::new(&notification.message_type)
                .with_source(notification.source_actor.as_deref().unwrap_or("unknown"))
                .with_payload(notification.payload)
        };

        debug!(
            primitive.name = %name,
            message_type = %msg.message_type,
            message_id = %msg.id,
            "received message"
        );

        if tx.send(msg).await.is_err() {
            warn!(
                primitive.name = %name,
                "message stream send failed, receiver dropped"
            );
            break;
        }
    }

    debug!(primitive.name = %name, "push bridge stopped");
}

/// Subscribe on an `IpcClient` and return a `MessageStream`.
///
/// Shared implementation used by both Handler and Sink.
async fn subscribe_and_stream(
    client: &IpcClient,
    topics: Vec<String>,
    name: &str,
    shutdown_kind: &str,
) -> Result<(MessageStream, Vec<String>)> {
    // Add system.shutdown to subscriptions (SDK handles it internally)
    let mut all_types = topics;
    if !all_types.iter().any(|t| t == "system.shutdown") {
        all_types.push("system.shutdown".to_string());
    }

    // Subscribe via IpcClient (single connection, no new socket)
    let sub_response = client
        .subscribe(all_types)
        .await
        .map_err(|e| ClientError::SubscriptionFailed(format!("subscribe failed: {e}")))?;

    if !sub_response.success {
        return Err(ClientError::SubscriptionFailed(
            sub_response
                .error
                .unwrap_or_else(|| "unknown error".to_string()),
        ));
    }

    // Take the push receiver and bridge to MessageStream
    let push_rx = client.take_push_receiver().ok_or_else(|| {
        ClientError::SubscriptionFailed(
            "push receiver already taken (subscribe called more than once?)".to_string(),
        )
    })?;

    let (tx, rx) = mpsc::channel(256);
    let bridge_name = name.to_string();
    let bridge_kind = shutdown_kind.to_string();

    tokio::spawn(async move {
        push_to_message_stream(push_rx, tx, bridge_name, &bridge_kind).await;
    });

    // Filter out system.shutdown from the reported subscriptions
    let user_subs: Vec<String> = sub_response
        .subscribed_types
        .into_iter()
        .filter(|s| s != "system.shutdown")
        .collect();

    Ok((MessageStream::new(rx), user_subs))
}

/// Query the engine for configured subscriptions via pub/sub pattern.
///
/// Uses `system.request.subscriptions` / `system.response.subscriptions`.
/// Requires a dedicated `IpcClient` connection since it temporarily subscribes
/// to the response topic.
async fn get_my_subscriptions_via_pubsub(name: &str) -> Result<Vec<String>> {
    debug!("querying configured subscriptions");

    let client = connect_to_engine(name, None).await?;
    let correlation_id = CorrelationId::new();

    // Subscribe to response type
    client
        .subscribe(vec!["system.response.subscriptions".to_string()])
        .await
        .map_err(|e| ClientError::SubscriptionFailed(format!("subscribe failed: {e}")))?;

    // Take push receiver before publishing
    let mut push_rx = client.take_push_receiver().ok_or_else(|| {
        ClientError::SubscriptionFailed("push receiver already taken".to_string())
    })?;

    // Publish request
    let request = EmergentMessage::new("system.request.subscriptions")
        .with_source(name)
        .with_correlation_id(correlation_id.clone())
        .with_payload(json!({ "name": name }));
    let envelope = build_publish_envelope(request)?;
    client
        .send(envelope)
        .await
        .map_err(|e| ClientError::ConnectionFailed(format!("publish failed: {e}")))?;

    // Wait for response with matching correlation_id
    let subs = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(notification) = push_rx.recv().await {
            if notification.message_type == "system.response.subscriptions" {
                let msg: EmergentMessage = serde_json::from_value(notification.payload)?;
                if msg.correlation_id.as_ref().map(|c| c.to_string())
                    == Some(correlation_id.to_string())
                {
                    let subs_response: SubscriptionsResponse = serde_json::from_value(msg.payload)?;
                    return Ok(subs_response.subscribes);
                }
            }
        }
        Err(ClientError::ConnectionFailed(
            "push channel closed before response".to_string(),
        ))
    })
    .await
    .map_err(|_| ClientError::Timeout)??;

    info!(types = ?subs, "received configured subscriptions");
    Ok(subs)
}

/// Query the engine for topology via pub/sub pattern.
///
/// Uses `system.request.topology` / `system.response.topology`.
async fn get_topology_via_pubsub(name: &str) -> Result<TopologyState> {
    debug!("querying topology");

    let client = connect_to_engine(name, None).await?;
    let correlation_id = CorrelationId::new();

    client
        .subscribe(vec!["system.response.topology".to_string()])
        .await
        .map_err(|e| ClientError::SubscriptionFailed(format!("subscribe failed: {e}")))?;

    let mut push_rx = client.take_push_receiver().ok_or_else(|| {
        ClientError::SubscriptionFailed("push receiver already taken".to_string())
    })?;

    let request = EmergentMessage::new("system.request.topology")
        .with_source(name)
        .with_correlation_id(correlation_id.clone())
        .with_payload(json!({}));
    let envelope = build_publish_envelope(request)?;
    client
        .send(envelope)
        .await
        .map_err(|e| ClientError::ConnectionFailed(format!("publish failed: {e}")))?;

    let state = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(notification) = push_rx.recv().await {
            if notification.message_type == "system.response.topology" {
                let msg: EmergentMessage = serde_json::from_value(notification.payload)?;
                if msg.correlation_id.as_ref().map(|c| c.to_string())
                    == Some(correlation_id.to_string())
                {
                    let topo_response: TopologyResponse = serde_json::from_value(msg.payload)?;
                    return Ok(TopologyState {
                        primitives: topo_response.primitives,
                    });
                }
            }
        }
        Err(ClientError::ConnectionFailed(
            "push channel closed before response".to_string(),
        ))
    })
    .await
    .map_err(|_| ClientError::Timeout)??;

    debug!(
        primitive_count = state.primitives.len(),
        "received topology"
    );
    Ok(state)
}

// ============================================================================
// Data Types
// ============================================================================

/// Response from `GetSubscriptions` request.
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

/// Response from `GetTopology` request.
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
    /// Channel-based IPC client (no mutex, no drain task).
    client: IpcClient,
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
        let client = connect_to_engine(name, None).await?;

        info!(primitive.name = %name, primitive.kind = "source", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client,
        })
    }

    /// Connect to the Emergent engine at a specific socket path.
    ///
    /// This is useful for testing or when connecting to a non-default socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails or the engine is not running.
    pub async fn connect_to(name: &str, socket_path: &std::path::Path) -> Result<Self> {
        let client = connect_to_engine(name, Some(socket_path)).await?;

        info!(primitive.name = %name, primitive.kind = "source", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client,
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

        let envelope = build_publish_envelope(message)?;
        self.client.send(envelope).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "failed to publish message");
            ClientError::ConnectionFailed(format!("publish failed: {e}"))
        })
    }

    /// Publish all messages from an iterator.
    ///
    /// Sends each message individually (no batching) so subscribers begin
    /// consuming immediately. Stops on the first error.
    ///
    /// Returns the number of messages successfully published.
    ///
    /// # Errors
    ///
    /// Returns the first publish error encountered.
    pub async fn publish_all(
        &self,
        messages: impl IntoIterator<Item = EmergentMessage>,
    ) -> Result<usize> {
        let mut count = 0;
        for message in messages {
            self.publish(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Publish messages from an async stream.
    ///
    /// Consumes the stream, publishing each message individually so subscribers
    /// begin consuming immediately. Stops on the first publish error or when
    /// the stream ends.
    ///
    /// Returns the number of messages successfully published.
    ///
    /// # Errors
    ///
    /// Returns the first publish error encountered.
    pub async fn publish_stream<S>(&self, mut stream: S) -> Result<usize>
    where
        S: futures::Stream<Item = EmergentMessage> + Unpin,
    {
        use futures::StreamExt;
        let mut count = 0;
        while let Some(message) = stream.next().await {
            self.publish(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        // Discovery uses a separate connection (request-response pattern)
        let client = connect_to_engine(&self.name, None).await?;
        let response = client
            .discover()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("discover failed: {e}")))?;

        if !response.success {
            return Err(ClientError::DiscoveryFailed(
                response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ));
        }

        let primitives = response
            .actors
            .unwrap_or_default()
            .into_iter()
            .map(|actor| PrimitiveInfo {
                name: actor.name,
                kind: String::new(),
            })
            .collect();

        let message_types = response.message_types.unwrap_or_default();

        Ok(DiscoveryInfo {
            message_types,
            primitives,
        })
    }

    /// Get the name of this source.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gracefully disconnect from the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");
        self.client
            .disconnect()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("disconnect failed: {e}")))?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
    }
}

// ============================================================================
// EmergentHandler - Subscribe + Publish
// ============================================================================

/// A Handler primitive that subscribes to and publishes messages.
///
/// Handlers are the transformation layer in a workflow. They receive messages
/// from Sources or other Handlers, process them, and emit new messages.
///
/// # Example
///
/// ```rust,ignore
/// use emergent_client::{EmergentHandler, EmergentMessage};
/// use futures::StreamExt;
///
/// let handler = EmergentHandler::connect("my_filter").await?;
/// let mut stream = handler.subscribe("timer.tick").await?;
///
/// while let Some(msg) = stream.next().await {
///     let output = EmergentMessage::new("timer.filtered")
///         .with_causation_id(msg.id());
///     handler.publish(output).await?;
/// }
/// ```
pub struct EmergentHandler {
    /// Name of this handler.
    name: String,
    /// Channel-based IPC client (no mutex, no drain task).
    client: Arc<IpcClient>,
    /// Currently subscribed message types.
    subscribed_types: Vec<String>,
}

impl EmergentHandler {
    /// Connect to the Emergent engine as a Handler.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(name: &str) -> Result<Self> {
        let client = connect_to_engine(name, None).await?;

        info!(primitive.name = %name, primitive.kind = "handler", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client: Arc::new(client),
            subscribed_types: Vec::new(),
        })
    }

    /// Connect to the Emergent engine at a specific socket path.
    ///
    /// This is useful for testing or when connecting to a non-default socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect_to(name: &str, socket_path: &std::path::Path) -> Result<Self> {
        let client = connect_to_engine(name, Some(socket_path)).await?;

        info!(primitive.name = %name, primitive.kind = "handler", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client: Arc::new(client),
            subscribed_types: Vec::new(),
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
    pub async fn subscribe(&mut self, types: impl IntoSubscription) -> Result<MessageStream> {
        let topics = types.into_topics();
        let (stream, user_subs) =
            subscribe_and_stream(&self.client, topics, &self.name, "handler").await?;
        self.subscribed_types = user_subs;
        Ok(stream)
    }

    /// Convenience method that connects, gets configured subscriptions, and returns
    /// (handler, stream) for the common one-liner pattern.
    ///
    /// # Errors
    ///
    /// Returns an error if connection or subscription fails.
    pub async fn messages(
        name: impl Into<String>,
        _types: impl IntoSubscription,
    ) -> Result<(Self, MessageStream)> {
        let name = name.into();
        let mut handler = Self::connect(&name).await?;
        let topics = get_my_subscriptions_via_pubsub(&name).await?;
        let stream = handler.subscribe(topics).await?;
        Ok((handler, stream))
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

        let envelope = build_publish_envelope(message)?;
        self.client.send(envelope).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "failed to publish message");
            ClientError::ConnectionFailed(format!("publish failed: {e}"))
        })
    }

    /// Publish all messages from an iterator.
    ///
    /// Sends each message individually (no batching) so subscribers begin
    /// consuming immediately. Stops on the first error.
    ///
    /// Returns the number of messages successfully published.
    ///
    /// # Errors
    ///
    /// Returns the first publish error encountered.
    pub async fn publish_all(
        &self,
        messages: impl IntoIterator<Item = EmergentMessage>,
    ) -> Result<usize> {
        let mut count = 0;
        for message in messages {
            self.publish(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Publish messages from an async stream.
    ///
    /// Consumes the stream, publishing each message individually so subscribers
    /// begin consuming immediately. Stops on the first publish error or when
    /// the stream ends.
    ///
    /// Returns the number of messages successfully published.
    ///
    /// # Errors
    ///
    /// Returns the first publish error encountered.
    pub async fn publish_stream<S>(&self, mut stream: S) -> Result<usize>
    where
        S: futures::Stream<Item = EmergentMessage> + Unpin,
    {
        use futures::StreamExt;
        let mut count = 0;
        while let Some(message) = stream.next().await {
            self.publish(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let client = connect_to_engine(&self.name, None).await?;
        let response = client
            .discover()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("discover failed: {e}")))?;

        if !response.success {
            return Err(ClientError::DiscoveryFailed(
                response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ));
        }

        let primitives = response
            .actors
            .unwrap_or_default()
            .into_iter()
            .map(|actor| PrimitiveInfo {
                name: actor.name,
                kind: String::new(),
            })
            .collect();

        let message_types = response.message_types.unwrap_or_default();

        Ok(DiscoveryInfo {
            message_types,
            primitives,
        })
    }

    /// Get the configured subscription types for this primitive.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_my_subscriptions(&self) -> Result<Vec<String>> {
        get_my_subscriptions_via_pubsub(&self.name).await
    }

    /// Get the name of this handler.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get currently subscribed message types.
    pub fn subscribed_types(&self) -> &[String] {
        &self.subscribed_types
    }

    /// Gracefully disconnect from the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");
        self.client
            .disconnect()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("disconnect failed: {e}")))?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
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
    /// Channel-based IPC client (no mutex, no drain task).
    client: Arc<IpcClient>,
    /// Currently subscribed message types.
    subscribed_types: Vec<String>,
}

impl EmergentSink {
    /// Connect to the Emergent engine as a Sink.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(name: &str) -> Result<Self> {
        let client = connect_to_engine(name, None).await?;

        info!(primitive.name = %name, primitive.kind = "sink", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client: Arc::new(client),
            subscribed_types: Vec::new(),
        })
    }

    /// Connect to the Emergent engine at a specific socket path.
    ///
    /// This is useful for testing or when connecting to a non-default socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect_to(name: &str, socket_path: &std::path::Path) -> Result<Self> {
        let client = connect_to_engine(name, Some(socket_path)).await?;

        info!(primitive.name = %name, primitive.kind = "sink", "connected to engine");

        Ok(Self {
            name: name.to_string(),
            client: Arc::new(client),
            subscribed_types: Vec::new(),
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
        let mut sink = Self::connect(&name).await?;
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
    pub async fn subscribe(&mut self, types: impl IntoSubscription) -> Result<MessageStream> {
        let topics = types.into_topics();
        let (stream, user_subs) =
            subscribe_and_stream(&self.client, topics, &self.name, "sink").await?;
        self.subscribed_types = user_subs;
        Ok(stream)
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let client = connect_to_engine(&self.name, None).await?;
        let response = client
            .discover()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("discover failed: {e}")))?;

        if !response.success {
            return Err(ClientError::DiscoveryFailed(
                response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ));
        }

        let primitives = response
            .actors
            .unwrap_or_default()
            .into_iter()
            .map(|actor| PrimitiveInfo {
                name: actor.name,
                kind: String::new(),
            })
            .collect();

        let message_types = response.message_types.unwrap_or_default();

        Ok(DiscoveryInfo {
            message_types,
            primitives,
        })
    }

    /// Get the configured subscription types for this primitive.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_my_subscriptions(&self) -> Result<Vec<String>> {
        get_my_subscriptions_via_pubsub(&self.name).await
    }

    /// Get the current topology (all primitives and their state).
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn get_topology(&self) -> Result<TopologyState> {
        get_topology_via_pubsub(&self.name).await
    }

    /// Get the name of this sink.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get currently subscribed message types.
    pub fn subscribed_types(&self) -> &[String] {
        &self.subscribed_types
    }

    /// Gracefully disconnect from the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        info!(primitive.name = %self.name, "disconnecting from engine");
        self.client
            .disconnect()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("disconnect failed: {e}")))?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
    }
}
