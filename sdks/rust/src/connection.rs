//! Connection types for Emergent primitives.
//!
//! This module provides the three primitive types that connect to the Emergent engine:
//! - [`EmergentSource`] - publish only
//! - [`EmergentHandler`] - subscribe and publish
//! - [`EmergentSink`] - subscribe only

use crate::error::ClientError;
use crate::message::EmergentMessage;
use crate::publish_watch::{PublishStats, PublishWatcher};
use crate::stream::MessageStream;
use crate::subscribe::{
    IntoSubscription, needs_configured_topics, partition_topics, resolve_topics,
};
use crate::types::{CorrelationId, MessageType, PrimitiveName};
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
/// No-op if the primitive already installed a subscriber: it keeps its own,
/// and no log directory or empty log file is created on its behalf.
fn init_tracing(name: &str) {
    use tracing_subscriber::EnvFilter;

    if tracing::dispatcher::has_been_set() {
        return;
    }

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

/// Decide whether `EMERGENT_UNWRAP_STDOUT` switches stdout unwrapping on.
///
/// Surrounding whitespace and letter case are ignored, and only `true` and `1`
/// enable it, the same rule as the Go, Python and TypeScript SDKs. Anything
/// else, an unset variable included, leaves it off.
fn parse_unwrap_flag(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// Extract the primitive kind a `system.shutdown` broadcast targets.
///
/// The engine forwards system events with the whole serialized
/// `EmergentMessage` as the notification payload, so the kind sits at
/// `payload.kind` inside that envelope. A bare `{"kind": ...}` object is
/// accepted too, which keeps a hand-written broadcast working. Returns `None`
/// when no string kind is present. The result is lowercased so callers can
/// compare it against a primitive kind directly.
fn extract_shutdown_kind(notification_payload: &serde_json::Value) -> Option<String> {
    fn kind_of(value: &serde_json::Value) -> Option<String> {
        value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(str::to_lowercase)
    }

    notification_payload
        .get("payload")
        .and_then(kind_of)
        .or_else(|| kind_of(notification_payload))
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

/// Build an `IpcEnvelope` for acknowledged publish (request-response).
///
/// The broker processes the message and returns a reply, providing backpressure.
fn build_publish_request_envelope(message: EmergentMessage) -> Result<IpcEnvelope> {
    let ipc_message = IpcEmergentMessage { inner: message };
    let payload = serde_json::to_value(&ipc_message)?;
    Ok(IpcEnvelope::new_request(
        "message_broker",
        "EmergentMessage",
        payload,
    ))
}

/// Whether a push notification names an Emergent message type.
///
/// A `*` subscription matches every IPC broadcast the engine makes, which
/// includes acton's own envelope names such as `SystemEvent`. Those are the
/// containers Emergent messages travel in, not messages in their own right,
/// and the engine forwards what they carry separately under its own type. They
/// are not valid Emergent message types, which is how they are told apart.
fn carries_emergent_message(message_type: &str) -> bool {
    MessageType::new(message_type).is_ok()
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

    let auto_unwrap = parse_unwrap_flag(std::env::var("EMERGENT_UNWRAP_STDOUT").ok().as_deref());

    while let Some(notification) = push_rx.recv().await {
        // Check for shutdown signal
        if notification.message_type == "system.shutdown" {
            let kind = extract_shutdown_kind(&notification.payload);
            info!(
                primitive.name = %name,
                shutdown_kind = %kind.as_deref().unwrap_or("unknown"),
                "received shutdown signal"
            );
            if kind
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case(shutdown_kind))
            {
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

        // Skip acton's own envelope broadcasts, which only a "*" subscription
        // ever sees. The message inside each one arrives separately under its
        // own Emergent message type.
        if !carries_emergent_message(&notification.message_type) {
            debug!(
                primitive.name = %name,
                message_type = %notification.message_type,
                "skipping non-Emergent IPC broadcast"
            );
            continue;
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

        // Auto-unwrap exec-source stdout payloads when configured
        let msg = if auto_unwrap && !msg.message_type.as_str().starts_with("system.") {
            msg.unwrap_stdout()
        } else {
            msg
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
///
/// Topics are split into literal message types and terminal-wildcard selectors
/// such as `tick.*` or `*`. The two travel as separate IPC requests because the
/// engine keeps two indexes, and a connection matching through both still
/// receives one copy of each message. A topic that could never match, such as
/// `system.*.error`, fails here before either request is sent.
async fn subscribe_and_stream(
    client: &IpcClient,
    topics: Vec<String>,
    name: &str,
    shutdown_kind: &str,
) -> Result<(MessageStream, Vec<String>)> {
    let partition = partition_topics(topics)?;

    // Add system.shutdown to subscriptions (SDK handles it internally)
    let mut all_types = partition.exact;
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

    let mut pattern_subs = Vec::new();
    if !partition.patterns.is_empty() {
        let pattern_response = client
            .subscribe_patterns(partition.patterns)
            .await
            .map_err(|e| {
                ClientError::SubscriptionFailed(format!("pattern subscribe failed: {e}"))
            })?;

        if !pattern_response.success {
            return Err(ClientError::SubscriptionFailed(
                pattern_response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ));
        }
        pattern_subs = pattern_response.subscribed_patterns;
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
        .chain(pattern_subs)
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
    client: Arc<IpcClient>,
    /// Claims the engine's reply to every fire-and-forget publish.
    watcher: PublishWatcher,
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

        Ok(Self::with_client(name, client))
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

        Ok(Self::with_client(name, client))
    }

    /// Assemble a connected Source and start watching its publishes.
    fn with_client(name: &str, client: IpcClient) -> Self {
        let client = Arc::new(client);
        let watcher = PublishWatcher::spawn(Arc::clone(&client), name.to_string());

        Self {
            name: name.to_string(),
            client,
            watcher,
        }
    }

    /// Publish a message to the engine (fire-and-forget).
    ///
    /// The message will be routed to any Handlers or Sinks subscribed to its type.
    ///
    /// # What `Ok(())` guarantees
    ///
    /// Only that the message was queued for the engine, in publish order. It
    /// does **not** mean the engine took it. The engine answers every publish,
    /// and the answer can be a refusal: the IPC connection is rate limited to
    /// 100 messages per second with a burst of 50, and the broker can also
    /// refuse a message because its mailbox is full or it is shutting down. A
    /// refused message is never delivered to anyone.
    ///
    /// After engine 0.13.1 that refusal is no longer silent. The SDK claims the
    /// engine's answer to every publish, logs a refusal at `WARN` with the
    /// engine's own error text and the message type, and counts it in
    /// [`publish_stats`](Self::publish_stats). Before that, `publish` reported
    /// success and the refusal was dropped at `trace` level.
    ///
    /// Use [`publish_ack`](Self::publish_ack) when the caller must learn the
    /// verdict of a specific message before moving on.
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be queued, which means the
    /// connection to the engine is gone.
    pub async fn publish(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let message_type = message.message_type.clone();
        let envelope = build_publish_envelope(message)?;
        self.watcher.publish(envelope, message_type).await
    }

    /// What the engine has done with this Source's fire-and-forget publishes.
    ///
    /// Counts of accepted, rejected and unanswered publishes since the Source
    /// connected. [`publish`](Self::publish) returns before the engine answers,
    /// so the counts lag the calls and settle once the queue drains.
    #[must_use]
    pub fn publish_stats(&self) -> PublishStats {
        self.watcher.stats()
    }

    /// Publish a message with broker acknowledgment (backpressure).
    ///
    /// # What `Ok(())` guarantees
    ///
    /// That the engine's message broker took this message: it stored the event
    /// and handed it to every subscriber's queue before replying. That is the
    /// strongest guarantee the SDK offers. It is not a delivery receipt from
    /// the subscribing primitives, which consume their queues on their own.
    ///
    /// A rejection, including a rate limit, comes back as
    /// [`ClientError::PublishFailed`] carrying the engine's own error text, so
    /// the caller decides what to do about it. That is the difference from
    /// [`publish`](Self::publish), which only logs and counts a rejection.
    ///
    /// The caller waits for a round trip per message, which is the
    /// backpressure: it cannot outpace the broker. Used internally by
    /// [`publish_all`](Self::publish_all) and
    /// [`publish_stream`](Self::publish_stream).
    ///
    /// # Errors
    ///
    /// Returns an error if the broker rejects the message or times out.
    pub async fn publish_ack(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let envelope = build_publish_request_envelope(message)?;
        // Keep publish order: this frame is written straight to the client, so
        // it must not overtake publishes still queued on the watcher.
        self.watcher.flush().await;
        let response = self.client.request(envelope).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "publish_ack failed");
            ClientError::ConnectionFailed(format!("publish_ack failed: {e}"))
        })?;
        if !response.success {
            return Err(ClientError::PublishFailed(
                response.error.unwrap_or_else(|| "broker error".to_string()),
            ));
        }
        Ok(())
    }

    /// Publish all messages from an iterator.
    ///
    /// Sends each message individually with broker acknowledgment so subscribers
    /// begin consuming immediately. Stops on the first error.
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
            self.publish_ack(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Publish messages from an async stream.
    ///
    /// Consumes the stream, publishing each message individually with broker
    /// acknowledgment so subscribers begin consuming immediately. Stops on the
    /// first publish error or when the stream ends.
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
            self.publish_ack(message).await?;
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
        self.watcher.shutdown().await;
        self.client
            .disconnect()
            .await
            .map_err(|e| ClientError::ConnectionFailed(format!("disconnect failed: {e}")))?;
        info!(primitive.name = %self.name, "disconnected from engine");
        Ok(())
    }

    /// Take the IPC push channel so a caller can watch for the engine's EOF.
    ///
    /// A Source subscribes to nothing, so this channel carries no messages it
    /// cares about. Its one useful event is closing: the client's reader task
    /// owns the sending half and drops it when the engine's socket reaches
    /// EOF, which is how a Source learns the engine died. Handlers and Sinks
    /// already get the same signal through their subscription stream.
    ///
    /// Returns `None` if the receiver was already taken.
    pub(crate) fn take_engine_push_channel(&self) -> Option<mpsc::Receiver<IpcPushNotification>> {
        self.client.take_push_receiver()
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
///
/// # Cloning
///
/// Cloning an `EmergentHandler` is cheap: the clone shares the same IPC
/// connection through an `Arc` and can publish independently. A clone carries
/// a snapshot of `subscribed_types` taken at the moment of the clone, so
/// subscribe on the original before cloning.
#[derive(Clone)]
pub struct EmergentHandler {
    /// Name of this handler.
    name: String,
    /// Channel-based IPC client (no mutex, no drain task).
    client: Arc<IpcClient>,
    /// Currently subscribed message types.
    subscribed_types: Vec<String>,
    /// Claims the engine's reply to every fire-and-forget publish.
    watcher: PublishWatcher,
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

        Ok(Self::with_client(name, client))
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

        Ok(Self::with_client(name, client))
    }

    /// Assemble a connected Handler and start watching its publishes.
    fn with_client(name: &str, client: IpcClient) -> Self {
        let client = Arc::new(client);
        let watcher = PublishWatcher::spawn(Arc::clone(&client), name.to_string());

        Self {
            name: name.to_string(),
            client,
            subscribed_types: Vec::new(),
            watcher,
        }
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

    /// Convenience method that connects, subscribes, and returns
    /// (handler, stream) for the common one-liner pattern.
    ///
    /// # Which topics are subscribed to
    ///
    /// The `types` you pass win. Pass an empty list to defer to the engine
    /// instead: the handler then queries its configured `subscribes` list from
    /// the engine's TOML and subscribes to that. The engine is only consulted
    /// when `types` is empty.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Subscribe to exactly these topics
    /// let (handler, stream) = EmergentHandler::messages("filter", ["timer.tick"]).await?;
    ///
    /// // Defer to the engine's configured `subscribes` list
    /// let (handler, stream) = EmergentHandler::messages("filter", Vec::<String>::new()).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if connection or subscription fails.
    pub async fn messages(
        name: impl Into<String>,
        types: impl IntoSubscription,
    ) -> Result<(Self, MessageStream)> {
        let name = name.into();
        let requested = types.into_topics();
        let configured = if needs_configured_topics(&requested) {
            get_my_subscriptions_via_pubsub(&name).await?
        } else {
            Vec::new()
        };
        let topics = resolve_topics(requested, configured);
        let mut handler = Self::connect(&name).await?;
        let stream = handler.subscribe(topics).await?;
        Ok((handler, stream))
    }

    /// Publish a message to the engine (fire-and-forget).
    ///
    /// # What `Ok(())` guarantees
    ///
    /// Only that the message was queued for the engine, in publish order. It
    /// does **not** mean the engine took it. The engine answers every publish,
    /// and the answer can be a refusal: the IPC connection is rate limited to
    /// 100 messages per second with a burst of 50, and the broker can also
    /// refuse a message because its mailbox is full or it is shutting down. A
    /// refused message is never delivered to anyone.
    ///
    /// After engine 0.13.1 that refusal is no longer silent. The SDK claims the
    /// engine's answer to every publish, logs a refusal at `WARN` with the
    /// engine's own error text and the message type, and counts it in
    /// [`publish_stats`](Self::publish_stats). Before that, `publish` reported
    /// success and the refusal was dropped at `trace` level.
    ///
    /// A Handler that emits one message per message it consumes stays under the
    /// rate limit as long as its input does. A Handler that fans one message out
    /// to many can exceed it, which is what [`publish_ack`](Self::publish_ack)
    /// or [`publish_all`](Self::publish_all) are for.
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be queued, which means the
    /// connection to the engine is gone.
    pub async fn publish(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let message_type = message.message_type.clone();
        let envelope = build_publish_envelope(message)?;
        self.watcher.publish(envelope, message_type).await
    }

    /// What the engine has done with this Handler's fire-and-forget publishes.
    ///
    /// Counts of accepted, rejected and unanswered publishes since the Handler
    /// connected. [`publish`](Self::publish) returns before the engine answers,
    /// so the counts lag the calls and settle once the queue drains.
    #[must_use]
    pub fn publish_stats(&self) -> PublishStats {
        self.watcher.stats()
    }

    /// Publish a message with broker acknowledgment (backpressure).
    ///
    /// # What `Ok(())` guarantees
    ///
    /// That the engine's message broker took this message: it stored the event
    /// and handed it to every subscriber's queue before replying. That is the
    /// strongest guarantee the SDK offers. It is not a delivery receipt from
    /// the subscribing primitives, which consume their queues on their own.
    ///
    /// A rejection, including a rate limit, comes back as
    /// [`ClientError::PublishFailed`] carrying the engine's own error text, so
    /// the caller decides what to do about it. That is the difference from
    /// [`publish`](Self::publish), which only logs and counts a rejection.
    ///
    /// The caller waits for a round trip per message, which is the
    /// backpressure: it cannot outpace the broker. Used internally by
    /// [`publish_all`](Self::publish_all) and
    /// [`publish_stream`](Self::publish_stream).
    ///
    /// # Errors
    ///
    /// Returns an error if the broker rejects the message or times out.
    pub async fn publish_ack(&self, mut message: EmergentMessage) -> Result<()> {
        if message.source.is_default() {
            message.source = PrimitiveName::new(&self.name).map_err(|e| {
                ClientError::ConnectionFailed(format!(
                    "invalid primitive name '{}': {}",
                    self.name, e
                ))
            })?;
        }

        let envelope = build_publish_request_envelope(message)?;
        // Keep publish order: this frame is written straight to the client, so
        // it must not overtake publishes still queued on the watcher.
        self.watcher.flush().await;
        let response = self.client.request(envelope).await.map_err(|e| {
            error!(primitive.name = %self.name, error = %e, "publish_ack failed");
            ClientError::ConnectionFailed(format!("publish_ack failed: {e}"))
        })?;
        if !response.success {
            return Err(ClientError::PublishFailed(
                response.error.unwrap_or_else(|| "broker error".to_string()),
            ));
        }
        Ok(())
    }

    /// Publish all messages from an iterator.
    ///
    /// Sends each message individually with broker acknowledgment so subscribers
    /// begin consuming immediately. Stops on the first error.
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
            self.publish_ack(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Publish messages from an async stream.
    ///
    /// Consumes the stream, publishing each message individually with broker
    /// acknowledgment so subscribers begin consuming immediately. Stops on the
    /// first publish error or when the stream ends.
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
            self.publish_ack(message).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Offer items as a pull-based stream with consumer-driven backpressure.
    ///
    /// Publishes `stream.ready`, then serves items one at a time as the
    /// consumer sends `stream.pull` requests. Publishes `stream.end`
    /// when exhausted.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream times out or the pull stream closes.
    pub async fn stream_offer(
        &self,
        message_type: &str,
        items: impl IntoIterator<Item = serde_json::Value>,
        pull_stream: &mut MessageStream,
        timeout: std::time::Duration,
    ) -> Result<usize> {
        let stream_id = CorrelationId::new().to_string();
        let mut items = items.into_iter();

        self.publish(
            EmergentMessage::new("stream.ready").with_payload(serde_json::json!({
                "stream_id": stream_id,
                "message_type": message_type,
            })),
        )
        .await?;

        let mut published = 0usize;

        loop {
            let msg = tokio::time::timeout(timeout, pull_stream.next())
                .await
                .map_err(|_| ClientError::Timeout)?
                .ok_or_else(|| {
                    ClientError::ConnectionFailed(
                        "pull stream closed during stream_offer".to_string(),
                    )
                })?;

            let is_pull = msg.message_type.as_str() == "stream.pull"
                && msg.payload.get("stream_id").and_then(|v| v.as_str())
                    == Some(stream_id.as_str());

            if is_pull {
                if let Some(item) = items.next() {
                    self.publish(
                        EmergentMessage::new(message_type)
                            .with_payload(item)
                            .with_metadata(serde_json::json!({"stream_id": stream_id})),
                    )
                    .await?;
                    published += 1;
                } else {
                    self.publish(
                        EmergentMessage::new("stream.end")
                            .with_payload(serde_json::json!({"stream_id": stream_id})),
                    )
                    .await?;
                    break;
                }
            }
        }

        Ok(published)
    }

    /// Consume a pull-based stream, yielding items via callback.
    ///
    /// Waits for `stream.ready`, then sends `stream.pull` requests
    /// automatically after each item is consumed. Stops on `stream.end`.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream times out or the source stream closes.
    pub async fn stream_consume(
        &self,
        message_type: &str,
        source_stream: &mut MessageStream,
        timeout: std::time::Duration,
        mut on_item: impl FnMut(EmergentMessage),
    ) -> Result<usize> {
        let stream_id = loop {
            let msg = tokio::time::timeout(timeout, source_stream.next())
                .await
                .map_err(|_| ClientError::Timeout)?
                .ok_or_else(|| {
                    ClientError::ConnectionFailed(
                        "source stream closed before stream.ready".to_string(),
                    )
                })?;

            if msg.message_type.as_str() == "stream.ready"
                && let (Some(mt), Some(sid)) = (
                    msg.payload.get("message_type").and_then(|v| v.as_str()),
                    msg.payload.get("stream_id").and_then(|v| v.as_str()),
                )
                && mt == message_type
            {
                break sid.to_string();
            }
        };

        self.publish(
            EmergentMessage::new("stream.pull")
                .with_payload(serde_json::json!({"stream_id": stream_id})),
        )
        .await?;

        let mut count = 0usize;

        loop {
            let msg = tokio::time::timeout(timeout, source_stream.next())
                .await
                .map_err(|_| ClientError::Timeout)?
                .ok_or_else(|| {
                    ClientError::ConnectionFailed(
                        "source stream closed during stream_consume".to_string(),
                    )
                })?;

            if msg.message_type.as_str() == "stream.end"
                && msg.payload.get("stream_id").and_then(|v| v.as_str()) == Some(stream_id.as_str())
            {
                break;
            }

            let is_item = msg.message_type.as_str() == message_type
                && msg
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("stream_id"))
                    .and_then(|v| v.as_str())
                    == Some(stream_id.as_str());

            if is_item {
                on_item(msg);
                count += 1;
                self.publish(
                    EmergentMessage::new("stream.pull")
                        .with_payload(serde_json::json!({"stream_id": stream_id})),
                )
                .await?;
            }
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
        self.watcher.shutdown().await;
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

    /// Convenience method that connects, subscribes, and returns a stream.
    ///
    /// This is a one-liner for the common pattern of:
    /// 1. Connect to the engine
    /// 2. Decide which topics to subscribe to
    /// 3. Subscribe to those topics
    /// 4. Return the message stream
    ///
    /// # Which topics are subscribed to
    ///
    /// The `types` you pass win. Pass an empty list to defer to the engine
    /// instead: the sink then queries its configured `subscribes` list from the
    /// engine's TOML and subscribes to that. The engine is only consulted when
    /// `types` is empty.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use futures::StreamExt;
    ///
    /// // Subscribe to exactly these topics
    /// let mut stream = EmergentSink::messages("console", ["timer.tick"]).await?;
    /// while let Some(msg) = stream.next().await {
    ///     println!("{}", msg.payload);
    /// }
    ///
    /// // Defer to the engine's configured `subscribes` list
    /// let mut stream = EmergentSink::messages("console", Vec::<String>::new()).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if connection or subscription fails.
    pub async fn messages(
        name: impl Into<String>,
        types: impl IntoSubscription,
    ) -> Result<MessageStream> {
        let name = name.into();
        let requested = types.into_topics();
        let mut sink = Self::connect(&name).await?;
        let configured = if needs_configured_topics(&requested) {
            sink.get_my_subscriptions().await?
        } else {
            Vec::new()
        };
        let topics = resolve_topics(requested, configured);
        let client = Arc::clone(&sink.client);
        // The sink itself is dropped when this function returns, so hand its
        // connection to the stream, which is all the caller gets back.
        Ok(sink.subscribe(topics).await?.owning(client))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The same table runs in the Go, Python and TypeScript SDKs.
    #[test]
    fn unwrap_flag_is_trimmed_and_case_insensitive() {
        let cases: [(Option<&str>, bool); 19] = [
            (Some("true"), true),
            (Some("1"), true),
            (Some("TRUE"), true),
            (Some("True"), true),
            (Some(" true "), true),
            (Some(" 1 "), true),
            (Some("\ttrue\n"), true),
            (None, false),
            (Some(""), false),
            (Some(" "), false),
            (Some("false"), false),
            (Some("0"), false),
            (Some("no"), false),
            (Some("off"), false),
            (Some("yes"), false),
            (Some("on"), false),
            (Some("2"), false),
            (Some("11"), false),
            (Some("truee"), false),
        ];
        for (value, want) in cases {
            assert_eq!(parse_unwrap_flag(value), want, "value: {value:?}");
        }
    }

    /// A `system.shutdown` notification payload exactly as the engine sends it:
    /// the whole serialized `EmergentMessage`, with the kind one level in.
    fn engine_shutdown_envelope(kind: &str) -> serde_json::Value {
        json!({
            "id": "msg_01m2xskqtyffgve4n1yw9vr8kz",
            "message_type": "system.shutdown",
            "source": "emergent",
            "timestamp_ms": 1_758_318_000_000_u64,
            "payload": { "kind": kind }
        })
    }

    fn shutdown_notification(kind: &str) -> IpcPushNotification {
        IpcPushNotification::new(
            "system.shutdown",
            Some("emergent".to_string()),
            engine_shutdown_envelope(kind),
        )
    }

    /// Await a stream read under a deadline, so a regression fails the test
    /// instead of hanging it.
    async fn next_within(
        stream: &mut MessageStream,
    ) -> std::result::Result<Option<EmergentMessage>, &'static str> {
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .map_err(|_| "timed out waiting on the message stream")
    }

    fn message_notification(message_type: &str) -> IpcPushNotification {
        let message = EmergentMessage::new(message_type).with_source("ticker");
        IpcPushNotification::new(
            message_type,
            Some("ticker".to_string()),
            serde_json::to_value(&message).unwrap_or_default(),
        )
    }

    #[test]
    fn extracts_the_kind_from_the_engine_envelope() {
        assert_eq!(
            extract_shutdown_kind(&engine_shutdown_envelope("sink")).as_deref(),
            Some("sink")
        );
    }

    #[test]
    fn extracts_the_kind_from_a_bare_kind_object() {
        assert_eq!(
            extract_shutdown_kind(&json!({ "kind": "handler" })).as_deref(),
            Some("handler")
        );
    }

    #[test]
    fn lowercases_the_extracted_kind() {
        assert_eq!(
            extract_shutdown_kind(&engine_shutdown_envelope("SINK")).as_deref(),
            Some("sink")
        );
    }

    #[test]
    fn prefers_the_inner_envelope_over_an_outer_kind() {
        let payload = json!({
            "kind": "source",
            "payload": { "kind": "sink" }
        });
        assert_eq!(extract_shutdown_kind(&payload).as_deref(), Some("sink"));
    }

    #[test]
    fn reports_no_kind_when_the_payload_carries_none() {
        for payload in [
            json!({}),
            json!({ "payload": {} }),
            json!({ "payload": { "kind": 7 } }),
            json!({ "kind": null }),
            json!("sink"),
            json!(null),
        ] {
            assert_eq!(extract_shutdown_kind(&payload), None, "payload: {payload}");
        }
    }

    #[tokio::test]
    async fn matching_shutdown_ends_the_message_stream() {
        let (push_tx, push_rx) = mpsc::channel(8);
        let (tx, rx) = mpsc::channel(8);
        let mut stream = MessageStream::new(rx);

        let bridge = tokio::spawn(push_to_message_stream(
            push_rx,
            tx,
            "printer".to_string(),
            "sink",
        ));

        push_tx
            .send(message_notification("timer.tick"))
            .await
            .map_err(|e| e.to_string())
            .ok();
        push_tx
            .send(shutdown_notification("sink"))
            .await
            .map_err(|e| e.to_string())
            .ok();

        let first = next_within(&mut stream).await;
        assert_eq!(
            first.map(|msg| msg.map(|msg| msg.message_type.to_string())),
            Ok(Some("timer.tick".to_string()))
        );
        assert_eq!(
            next_within(&mut stream).await.map(|msg| msg.is_none()),
            Ok(true),
            "stream should end on a shutdown that targets this kind"
        );
        assert!(bridge.await.is_ok());
    }

    #[tokio::test]
    async fn shutdown_for_another_kind_leaves_the_stream_open() {
        let (push_tx, push_rx) = mpsc::channel(8);
        let (tx, rx) = mpsc::channel(8);
        let mut stream = MessageStream::new(rx);

        let bridge = tokio::spawn(push_to_message_stream(
            push_rx,
            tx,
            "printer".to_string(),
            "sink",
        ));

        push_tx
            .send(shutdown_notification("source"))
            .await
            .map_err(|e| e.to_string())
            .ok();
        push_tx
            .send(shutdown_notification("handler"))
            .await
            .map_err(|e| e.to_string())
            .ok();
        push_tx
            .send(message_notification("timer.tick"))
            .await
            .map_err(|e| e.to_string())
            .ok();

        let next = next_within(&mut stream).await;
        assert_eq!(
            next.map(|msg| msg.map(|msg| msg.message_type.to_string())),
            Ok(Some("timer.tick".to_string())),
            "a shutdown for another kind must not end the stream"
        );

        drop(push_tx);
        assert!(bridge.await.is_ok());
    }

    #[test]
    fn emergent_message_types_are_carried_through() {
        assert!(carries_emergent_message("tick.out"));
        assert!(carries_emergent_message("system.started.ticker"));
        assert!(carries_emergent_message("system.shutdown.requested"));
    }

    #[test]
    fn acton_envelope_names_are_not_emergent_messages() {
        assert!(!carries_emergent_message("SystemEvent"));
        assert!(!carries_emergent_message("EmergentMessage"));
        assert!(!carries_emergent_message(""));
    }
}
