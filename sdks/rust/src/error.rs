//! Error types for the Emergent client library.

use acton_reactive::ipc::IpcError;
use thiserror::Error;

/// Client errors.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Connection failed.
    #[error("Failed to connect to engine: {0}")]
    ConnectionFailed(String),

    /// Socket not found.
    #[error("Engine socket not found at: {0}")]
    SocketNotFound(String),

    /// I/O error during communication.
    #[error("Communication error: {0}")]
    IoError(#[from] std::io::Error),

    /// IPC error.
    #[error("IPC error: {0}")]
    IpcError(String),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Subscription failed.
    #[error("Subscription failed: {0}")]
    SubscriptionFailed(String),

    /// Publish failed.
    #[error("Publish failed: {0}")]
    PublishFailed(String),

    /// Discovery failed.
    #[error("Discovery failed: {0}")]
    DiscoveryFailed(String),

    /// Request timed out.
    #[error("Request timed out")]
    Timeout,

    /// Engine returned an error.
    #[error("Engine error: {0}")]
    EngineError(String),

    /// Protocol error.
    #[error("Protocol error: {0}")]
    ProtocolError(String),
}

impl From<IpcError> for ClientError {
    fn from(e: IpcError) -> Self {
        ClientError::IpcError(e.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        ClientError::SerializationError(e.to_string())
    }
}

impl From<rmp_serde::encode::Error> for ClientError {
    fn from(e: rmp_serde::encode::Error) -> Self {
        ClientError::SerializationError(e.to_string())
    }
}

impl From<rmp_serde::decode::Error> for ClientError {
    fn from(e: rmp_serde::decode::Error) -> Self {
        ClientError::SerializationError(e.to_string())
    }
}
