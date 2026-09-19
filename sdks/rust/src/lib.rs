//! Emergent Client Library
//!
//! This crate provides the client-side SDK for building Sources, Handlers, and Sinks
//! that connect to the Emergent workflow engine.
//!
//! # Primitives
//!
//! Emergent uses three primitives that define how clients interact with the message bus:
//!
//! - [`EmergentSource`] - Publishes messages to the workflow (ingress from external world)
//! - [`EmergentHandler`] - Subscribes to and publishes messages (transformation/processing)
//! - [`EmergentSink`] - Subscribes to messages (egress to external world)
//!
//! # Example: Source
//!
//! ```rust,ignore
//! use emergent_client::{EmergentSource, EmergentMessage};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let source = EmergentSource::connect("my_source").await?;
//!
//!     let message = EmergentMessage::new("timer.tick")
//!         .with_payload(json!({"sequence": 1}));
//!
//!     source.publish(message).await?;
//!     Ok(())
//! }
//! ```
//!
//! # Example: Handler
//!
//! ```rust,ignore
//! use emergent_client::{EmergentHandler, EmergentMessage};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let handler = EmergentHandler::connect("my_handler").await?;
//!     let mut stream = handler.subscribe(&["timer.tick"]).await?;
//!
//!     while let Some(msg) = stream.next().await {
//!         // Process and publish transformed message
//!         let output = EmergentMessage::new("timer.processed")
//!             .with_causation_id(msg.id())
//!             .with_payload(json!({"original": msg.payload}));
//!         handler.publish(output).await?;
//!     }
//!     Ok(())
//! }
//! ```
//!
//! # Example: Sink
//!
//! ```rust,ignore
//! use emergent_client::EmergentSink;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let sink = EmergentSink::connect("my_sink").await?;
//!     let mut stream = sink.subscribe(&["timer.processed"]).await?;
//!
//!     while let Some(msg) = stream.next().await {
//!         println!("Received: {} - {:?}", msg.message_type, msg.payload);
//!     }
//!     Ok(())
//! }
//! ```

mod connection;
mod error;
pub mod helpers;
mod message;
pub mod prelude;
mod stream;
pub mod subscribe;
pub mod types;

pub use connection::{
    EmergentHandler, EmergentSink, EmergentSource, TopologyPrimitive, TopologyState,
};
pub use error::ClientError;
pub use message::{EmergentMessage, create_message};
pub use stream::MessageStream;
pub use subscribe::{
    IntoSubscription, InvalidTopic, MAX_PATTERN_LEN, TopicKind, classify_topic, pattern_prefix,
    topic_matches,
};

/// The version of this SDK, taken from its own manifest at compile time.
///
/// `emergent scaffold` renders the dependency requirement of a generated
/// primitive from this constant, so a scaffolded crate always asks for the SDK
/// that the engine generating it was built against. The engine crate carries
/// its own version, which is why it cannot be the source of this string.
///
/// ```
/// let (major, rest) = emergent_client::VERSION
///     .split_once('.')
///     .unwrap_or_default();
/// assert!(major.parse::<u64>().is_ok());
/// assert!(rest.contains('.'));
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Result type for client operations.
pub type Result<T> = std::result::Result<T, ClientError>;

/// What the engine's IPC layer reports about itself.
///
/// These are acton IPC type names and IPC-exposed actors (`SystemEvent`,
/// `message_broker`), not Emergent topics or primitives. For those, ask a sink
/// for the topology or read `GET /api/topology`.
#[derive(Debug, Clone)]
pub struct DiscoveryInfo {
    /// IPC message type names the engine has registered.
    pub message_types: Vec<String>,
    /// Actors the engine exposes over IPC.
    pub primitives: Vec<PrimitiveInfo>,
}

/// One IPC-exposed actor from a discovery reply.
#[derive(Debug, Clone)]
pub struct PrimitiveInfo {
    /// Name of the actor.
    pub name: String,
    /// Always empty: the engine's discovery reply carries no kind.
    pub kind: String,
}
