//! Connection types for Emergent primitives.
//!
//! This module provides the three primitive types that connect to the Emergent engine:
//! - [`EmergentSource`] - publish only
//! - [`EmergentHandler`] - subscribe and publish
//! - [`EmergentSink`] - subscribe only

use crate::error::ClientError;
use crate::message::EmergentMessage;
use crate::stream::MessageStream;
use crate::{DiscoveryInfo, PrimitiveInfo, Result};

use acton_reactive::ipc::protocol::{
    read_frame, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_DISCOVER, MSG_TYPE_PUSH,
    MSG_TYPE_REQUEST, MSG_TYPE_RESPONSE, MSG_TYPE_SUBSCRIBE, MSG_TYPE_UNSUBSCRIBE,
};
use acton_reactive::ipc::{
    socket_exists, socket_is_alive, IpcConfig, IpcDiscoverRequest, IpcDiscoverResponse,
    IpcEnvelope, IpcPushNotification, IpcSubscribeRequest, IpcSubscriptionResponse,
    IpcUnsubscribeRequest,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex};
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

/// Connect to the engine socket with health checks.
async fn connect_to_engine(name: &str) -> Result<UnixStream> {
    let socket_path = resolve_socket_path(name)?;

    if !socket_exists(&socket_path) {
        return Err(ClientError::SocketNotFound(
            socket_path.display().to_string(),
        ));
    }

    if !socket_is_alive(&socket_path).await {
        return Err(ClientError::ConnectionFailed(
            "Engine socket exists but is not responding".to_string(),
        ));
    }

    UnixStream::connect(&socket_path)
        .await
        .map_err(|e| ClientError::ConnectionFailed(e.to_string()))
}

/// Send a discover request and get available message types.
async fn discover_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
) -> Result<DiscoveryInfo> {
    let request = IpcDiscoverRequest::new();
    let payload = rmp_serde::to_vec(&request)?;

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
            response.error.unwrap_or_else(|| "Unknown error".to_string()),
        ));
    }

    Ok(DiscoveryInfo {
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
    })
}

/// Subscribe to message types.
async fn subscribe_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    types: &[&str],
) -> Result<Vec<String>> {
    let request = IpcSubscribeRequest::new(types.iter().map(|s| (*s).to_string()).collect());
    let payload = rmp_serde::to_vec(&request)?;

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
            response.error.unwrap_or_else(|| "Unknown error".to_string()),
        ));
    }

    Ok(response.subscribed_types)
}

/// Unsubscribe from message types.
async fn unsubscribe_impl(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    types: &[&str],
) -> Result<()> {
    let request = if types.is_empty() {
        IpcUnsubscribeRequest::unsubscribe_all()
    } else {
        IpcUnsubscribeRequest::new(types.iter().map(|s| (*s).to_string()).collect())
    };
    let payload = rmp_serde::to_vec(&request)?;

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
        return Err(ClientError::SubscriptionFailed(
            response.error.unwrap_or_else(|| "Unknown error".to_string()),
        ));
    }

    Ok(())
}

/// Publish a message (fire-and-forget).
async fn publish_impl(writer: &mut OwnedWriteHalf, message: EmergentMessage) -> Result<()> {
    let ipc_message = IpcEmergentMessage { inner: message };
    let envelope = IpcEnvelope::new(
        "message_broker",
        "EmergentMessage",
        serde_json::to_value(&ipc_message)?,
    );

    let payload = rmp_serde::to_vec(&envelope)?;
    write_frame(writer, MSG_TYPE_REQUEST, Format::MessagePack, &payload)
        .await
        .map_err(ClientError::from)?;

    Ok(())
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
    /// Reader half (kept for discovery, but generally unused).
    reader: Arc<Mutex<OwnedReadHalf>>,
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

        Ok(Self {
            name: name.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
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
        // Set the source if not already set
        if message.source.is_empty() {
            message.source = self.name.clone();
        }

        let mut writer = self.writer.lock().await;
        publish_impl(&mut writer, message).await
    }

    /// Discover available message types and primitives.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails.
    pub async fn discover(&self) -> Result<DiscoveryInfo> {
        let mut reader = self.reader.lock().await;
        let mut writer = self.writer.lock().await;
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
        // Send unsubscribe-all as a "goodbye" signal on the original connection.
        // We use the original connection (not a new one) so the server knows
        // THIS specific connection is closing.
        let mut reader = self.reader.lock().await;
        let mut writer = self.writer.lock().await;

        let request = IpcUnsubscribeRequest::unsubscribe_all();
        let payload = rmp_serde::to_vec(&request)?;

        // Send the unsubscribe request
        if write_frame(&mut *writer, MSG_TYPE_UNSUBSCRIBE, Format::MessagePack, &payload)
            .await
            .is_err()
        {
            // Connection already closed, that's fine for disconnect
            return Ok(());
        }

        // Wait for response with a short timeout to let the server finish processing.
        let short_timeout = Duration::from_secs(1);
        let _ = timeout(short_timeout, read_frame(&mut *reader, MAX_FRAME_SIZE)).await;

        // Explicitly shutdown the writer for clean socket close
        use tokio::io::AsyncWriteExt;
        let _ = writer.shutdown().await;

        Ok(())
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
}

impl EmergentHandler {
    /// Connect to the Emergent engine as a Handler.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(name: &str) -> Result<Self> {
        let stream = connect_to_engine(name).await?;
        let (_reader, writer) = stream.into_split();

        Ok(Self {
            name: name.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            subscribed_types: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Subscribe to message types and return a stream of incoming messages.
    ///
    /// The SDK automatically handles `system.shutdown` messages - when the engine
    /// signals shutdown for handlers, the stream will close gracefully.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription fails.
    pub async fn subscribe(&self, types: &[&str]) -> Result<MessageStream> {
        // Create a new connection for receiving (subscriptions need dedicated reader)
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        // Add system.shutdown to subscriptions (SDK handles it internally)
        let mut all_types: Vec<&str> = types.to_vec();
        if !all_types.contains(&"system.shutdown") {
            all_types.push("system.shutdown");
        }

        // Subscribe
        let subscribed = subscribe_impl(&mut reader, &mut writer, &all_types).await?;

        // Update tracked subscriptions (excluding internal system.shutdown)
        {
            let mut subs = self.subscribed_types.lock().await;
            *subs = subscribed.into_iter().filter(|s| s != "system.shutdown").collect();
        }

        // Create channel for message stream
        let (tx, rx) = mpsc::channel(256);

        // Spawn task to read push notifications
        // IMPORTANT: We must keep the writer alive even though we don't use it.
        // Dropping it would half-close the socket and cause the server to close the connection.
        tokio::spawn(async move {
            // Keep writer alive by moving it into the task (but don't use it)
            let _writer = writer;

            loop {
                match read_frame(&mut reader, MAX_FRAME_SIZE).await {
                    Ok((msg_type, _, payload)) => {
                        if msg_type == MSG_TYPE_PUSH {
                            match rmp_serde::from_slice::<IpcPushNotification>(&payload) {
                                Ok(notification) => {
                                    // Check for shutdown signal
                                    if notification.message_type == "system.shutdown" {
                                        if let Some(kind) = notification.payload.get("kind").and_then(|v| v.as_str()) {
                                            if kind == "handler" {
                                                // Graceful shutdown - close the stream
                                                break;
                                            }
                                        }
                                        continue; // Don't forward system.shutdown to user
                                    }

                                    // Try to extract EmergentMessage from payload
                                    // The broker sends EmergentMessage directly as the payload
                                    if let Ok(msg) = serde_json::from_value::<EmergentMessage>(
                                        notification.payload.clone(),
                                    ) {
                                        if tx.send(msg).await.is_err() {
                                            break; // Receiver dropped
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
                                        if tx.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse push notification: {}", e);
                                }
                            }
                        }
                        // Ignore non-PUSH frames (heartbeats, etc.)
                    }
                    Err(e) => {
                        tracing::debug!("Connection closed: {}", e);
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
        if message.source.is_empty() {
            message.source = self.name.clone();
        }

        let mut writer = self.writer.lock().await;
        publish_impl(&mut writer, message).await
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
        // Unsubscribe from all message types
        self.unsubscribe(&[]).await?;
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

        Ok(Self {
            name: name.to_string(),
            subscribed_types: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Subscribe to message types and return a stream of incoming messages.
    ///
    /// The SDK automatically handles `system.shutdown` messages - when the engine
    /// signals shutdown for sinks, the stream will close gracefully.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription fails.
    pub async fn subscribe(&self, types: &[&str]) -> Result<MessageStream> {
        let stream = connect_to_engine(&self.name).await?;
        let (mut reader, mut writer) = stream.into_split();

        // Add system.shutdown to subscriptions (SDK handles it internally)
        let mut all_types: Vec<&str> = types.to_vec();
        if !all_types.contains(&"system.shutdown") {
            all_types.push("system.shutdown");
        }

        // Subscribe
        let subscribed = subscribe_impl(&mut reader, &mut writer, &all_types).await?;

        // Update tracked subscriptions (excluding internal system.shutdown)
        {
            let mut subs = self.subscribed_types.lock().await;
            *subs = subscribed.into_iter().filter(|s| s != "system.shutdown").collect();
        }

        // Create channel for message stream
        let (tx, rx) = mpsc::channel(256);

        // Spawn task to read push notifications
        // IMPORTANT: We must keep the writer alive even though we don't use it.
        // Dropping it would half-close the socket and cause the server to close the connection.
        tokio::spawn(async move {
            // Keep writer alive by moving it into the task (but don't use it)
            let _writer = writer;

            loop {
                match read_frame(&mut reader, MAX_FRAME_SIZE).await {
                    Ok((msg_type, _, payload)) => {
                        if msg_type == MSG_TYPE_PUSH {
                            match rmp_serde::from_slice::<IpcPushNotification>(&payload) {
                                Ok(notification) => {
                                    // Check for shutdown signal
                                    if notification.message_type == "system.shutdown" {
                                        if let Some(kind) = notification.payload.get("kind").and_then(|v| v.as_str()) {
                                            if kind == "sink" {
                                                // Graceful shutdown - close the stream
                                                break;
                                            }
                                        }
                                        continue; // Don't forward system.shutdown to user
                                    }

                                    // Try to extract EmergentMessage from payload
                                    // The broker sends EmergentMessage directly as the payload
                                    if let Ok(msg) = serde_json::from_value::<EmergentMessage>(
                                        notification.payload.clone(),
                                    ) {
                                        if tx.send(msg).await.is_err() {
                                            break; // Receiver dropped
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
                                        if tx.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse push notification: {}", e);
                                }
                            }
                        }
                        // Ignore non-PUSH frames (heartbeats, etc.)
                    }
                    Err(e) => {
                        tracing::debug!("Connection closed: {}", e);
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
        // Unsubscribe from all message types
        self.unsubscribe(&[]).await?;
        Ok(())
    }
}
