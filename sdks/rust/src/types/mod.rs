//! Core types for Emergent messages and primitives.
//!
//! This module provides strongly-typed newtypes for all identifiers and
//! domain values used in the Emergent system.
//!
//! # TypeID Identifiers
//!
//! These types use the TypeID format (`prefix_<uuid_v7>`) for unique,
//! time-sortable, and self-describing identifiers:
//!
//! - [`MessageId`] - Unique message identifier (`msg_<uuid_v7>`)
//! - [`CorrelationId`] - Request-response tracking (`cor_<uuid_v7>`)
//! - [`CausationId`] - Causation chain tracking (`cau_<uuid_v7>` or `msg_<uuid_v7>`)
//!
//! # Value Objects
//!
//! These types provide validation and type safety for domain values:
//!
//! - [`MessageType`] - Validated message type (e.g., "timer.tick")
//! - [`PrimitiveName`] - Validated primitive name (e.g., "timer_source")
//! - [`Timestamp`] - Milliseconds since Unix epoch
//!
//! # Example
//!
//! ```rust
//! use emergent_client::types::{MessageId, MessageType, PrimitiveName, Timestamp};
//!
//! // Create typed identifiers
//! let id = MessageId::new();
//! let msg_type = MessageType::new("timer.tick").expect("valid type");
//! let name = PrimitiveName::new("timer").expect("valid name");
//! let ts = Timestamp::now();
//!
//! // Types are self-describing and serializable
//! assert!(id.to_string().starts_with("msg_"));
//! assert_eq!(msg_type.as_str(), "timer.tick");
//! ```

mod causation_id;
mod correlation_id;
mod message_id;
mod message_type;
mod primitive_name;
mod timestamp;

pub use causation_id::{CausationId, InvalidCausationId};
pub use correlation_id::{CorrelationId, InvalidCorrelationId};
pub use message_id::{InvalidMessageId, MessageId};
pub use message_type::{InvalidMessageType, MessageType};
pub use primitive_name::{InvalidPrimitiveName, PrimitiveName};
pub use timestamp::Timestamp;
