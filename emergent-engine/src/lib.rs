//! Emergent Engine - Event-based workflow platform with emergent behaviors
//!
//! The Emergent Engine is the core runtime that manages Sources, Handlers, and Sinks
//! to create event-driven workflows with emergent behaviors.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      Emergent Engine                             │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
//! │  │  Process    │  │    IPC      │  │    Event Store          │  │
//! │  │  Manager    │  │   Server    │  │  (JSON logs + SQLite)   │  │
//! │  └─────────────┘  └─────────────┘  └─────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────┘
//!         │                 │                 │
//!         ▼                 ▼                 ▼
//!    ┌─────────┐      ┌───────────┐      ┌────────┐
//!    │ Sources │      │ Handlers  │      │ Sinks  │
//!    └─────────┘      └───────────┘      └────────┘
//! ```

pub mod config;
pub mod event_store;
pub mod marketplace;
pub mod messages;
pub mod primitive_actor;
pub mod primitives;
pub mod process_manager;
pub mod scaffold;

pub use config::EmergentConfig;
pub use messages::EmergentMessage;
pub use primitive_actor::{
    build_primitive_actor, IpcSystemEvent, PrimitiveActorConfig, PrimitiveActorState,
    SystemEventPayload,
};
pub use primitives::{PrimitiveKind, PrimitiveState};
