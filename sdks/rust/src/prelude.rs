//! Convenient re-exports for common usage.
//!
//! This module provides a single import for the most commonly used types:
//!
//! ```rust,ignore
//! use emergent_client::prelude::*;
//!
//! let sink = EmergentSink::connect("my_sink").await?;
//! let mut stream = sink.subscribe(["timer.tick"]).await?;
//!
//! while let Some(msg) = stream.next().await {
//!     let payload: MyPayload = msg.payload_as()?;
//!     println!("Received: {:?}", payload);
//! }
//! ```

pub use crate::connection::{EmergentHandler, EmergentSink, EmergentSource};
pub use crate::error::ClientError;
pub use crate::message::{create_message, EmergentMessage};
pub use crate::stream::MessageStream;
pub use crate::subscribe::IntoSubscription;
pub use crate::Result;

// Re-export StreamExt for convenient stream operations
pub use futures::StreamExt;
