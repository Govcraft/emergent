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
pub mod init;
pub mod lifecycle;
pub mod marketplace;
pub mod messages;
pub mod primitive_actor;
pub mod primitives;
pub mod process_manager;
pub mod publish_reply;
pub mod readiness;
pub mod retention;
pub mod scaffold;
pub mod supervision;
pub mod topology;
pub mod update;

pub use config::EmergentConfig;
pub use lifecycle::{LifecycleEvent, PrimitiveStatus, next_status};
pub use messages::EmergentMessage;
pub use primitive_actor::{
    IpcSystemEvent, PrimitiveActorConfig, PrimitiveActorState, SystemEventPayload,
    build_primitive_actor,
};
pub use primitives::{PrimitiveKind, PrimitiveState};
pub use publish_reply::should_reply;
pub use readiness::{
    Evidence, Ledger, ObservedSubscriber, StartupObserver, SubscribeSignals, TierMember,
    TierVerdict, absorb, attribute, classify, evaluate_tier,
};
pub use supervision::{
    ExitOutcome, RestartDecision, RestartLimits, RestartPolicy, backoff_delay, decide_restart,
};
pub use topology::{TopologyPrimitive, TopologyResponsePayload, build_topology_payload};
