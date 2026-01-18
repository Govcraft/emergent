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
mod subscribe;
pub mod types;

pub use connection::{EmergentHandler, EmergentSink, EmergentSource};
pub use error::ClientError;
pub use message::{create_message, EmergentMessage};
pub use stream::MessageStream;
pub use subscribe::IntoSubscription;

/// Result type for client operations.
pub type Result<T> = std::result::Result<T, ClientError>;

/// Discovery information about the engine.
#[derive(Debug, Clone)]
pub struct DiscoveryInfo {
    /// Available message types that can be subscribed to.
    pub message_types: Vec<String>,
    /// List of connected primitives.
    pub primitives: Vec<PrimitiveInfo>,
}

/// Information about a registered primitive.
#[derive(Debug, Clone)]
pub struct PrimitiveInfo {
    /// Name of the primitive.
    pub name: String,
    /// Type of primitive (Source, Handler, Sink).
    pub kind: String,
}
