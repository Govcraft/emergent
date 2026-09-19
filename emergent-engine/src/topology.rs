//! Topology response types and the pure mapping from process-manager state.
//!
//! The engine answers topology queries over two transports:
//!
//! - `GET /api/topology` over the HTTP API
//! - `system.request.topology` / `system.response.topology` over pub/sub
//!
//! Both transports serialize the exact same payload, built by
//! [`build_topology_payload`], so the two answers cannot drift apart.

use serde::{Deserialize, Serialize};

use crate::primitives::PrimitiveInfo;

/// The name the engine reports itself under in topology responses.
pub const ENGINE_PRIMITIVE_NAME: &str = "emergent-engine";

/// Information about a primitive in the topology response.
///
/// Used both in `GET /api/topology` bodies and in
/// `system.response.topology` message payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyPrimitive {
    /// Unique name of the primitive.
    pub name: String,
    /// Kind of primitive (source, handler, sink).
    pub kind: String,
    /// Current state (running, stopped, failed, etc.).
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

/// Payload for topology responses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyResponsePayload {
    /// All primitives in the system, engine first.
    pub primitives: Vec<TopologyPrimitive>,
}

/// Build the engine's own entry in the topology.
///
/// The engine is reported as a source because it only publishes: it emits the
/// lifecycle events that primitives subscribe to.
#[must_use]
pub fn engine_primitive(engine_pid: u32) -> TopologyPrimitive {
    TopologyPrimitive {
        name: ENGINE_PRIMITIVE_NAME.to_string(),
        kind: "source".to_string(),
        state: "running".to_string(),
        publishes: vec![
            "system.started.*".to_string(),
            "system.stopped.*".to_string(),
            "system.error.*".to_string(),
            "system.shutdown".to_string(),
        ],
        subscribes: Vec::new(),
        pid: Some(engine_pid),
        error: None,
    }
}

/// Map process-manager state into a topology response payload.
///
/// This is a pure function: given the engine PID and the primitives the process
/// manager knows about, it returns the payload both transports serialize. The
/// engine itself is always the first entry.
#[must_use]
pub fn build_topology_payload(
    engine_pid: u32,
    primitives: Vec<PrimitiveInfo>,
) -> TopologyResponsePayload {
    let mut out = Vec::with_capacity(primitives.len() + 1);
    out.push(engine_primitive(engine_pid));
    out.extend(primitives.into_iter().map(|p| TopologyPrimitive {
        name: p.name,
        kind: p.kind.to_string().to_lowercase(),
        state: p.state.to_string().to_lowercase(),
        publishes: p.publishes,
        subscribes: p.subscribes,
        pid: p.pid,
        error: p.error,
    }));
    TopologyResponsePayload { primitives: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{PrimitiveKind, PrimitiveState};

    fn info(
        name: &str,
        kind: PrimitiveKind,
        state: PrimitiveState,
        publishes: &[&str],
        subscribes: &[&str],
    ) -> PrimitiveInfo {
        PrimitiveInfo {
            name: name.to_string(),
            kind,
            state,
            publishes: publishes.iter().map(|s| (*s).to_string()).collect(),
            subscribes: subscribes.iter().map(|s| (*s).to_string()).collect(),
            pid: None,
            error: None,
        }
    }

    #[test]
    fn engine_is_always_first_and_present_when_empty() {
        let payload = build_topology_payload(42, Vec::new());
        assert_eq!(payload.primitives.len(), 1);
        let engine = &payload.primitives[0];
        assert_eq!(engine.name, ENGINE_PRIMITIVE_NAME);
        assert_eq!(engine.kind, "source");
        assert_eq!(engine.state, "running");
        assert_eq!(engine.pid, Some(42));
        assert!(engine.subscribes.is_empty());
        assert!(engine.error.is_none());
        assert!(engine.publishes.contains(&"system.shutdown".to_string()));
    }

    #[test]
    fn primitives_follow_the_engine_in_order() {
        let payload = build_topology_payload(
            1,
            vec![
                info(
                    "timer",
                    PrimitiveKind::Source,
                    PrimitiveState::Running,
                    &["timer.tick"],
                    &[],
                ),
                info(
                    "console",
                    PrimitiveKind::Sink,
                    PrimitiveState::Configured,
                    &[],
                    &["timer.tick"],
                ),
            ],
        );

        let names: Vec<&str> = payload.primitives.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec![ENGINE_PRIMITIVE_NAME, "timer", "console"]);
    }

    #[test]
    fn kind_and_state_are_lowercased() {
        let payload = build_topology_payload(
            1,
            vec![
                info(
                    "h",
                    PrimitiveKind::Handler,
                    PrimitiveState::Running,
                    &[],
                    &[],
                ),
                info("s", PrimitiveKind::Sink, PrimitiveState::Stopped, &[], &[]),
                info(
                    "e",
                    PrimitiveKind::Source,
                    PrimitiveState::External,
                    &[],
                    &[],
                ),
            ],
        );

        let kinds: Vec<&str> = payload.primitives[1..]
            .iter()
            .map(|p| p.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["handler", "sink", "source"]);

        let states: Vec<&str> = payload.primitives[1..]
            .iter()
            .map(|p| p.state.as_str())
            .collect();
        assert_eq!(states, vec!["running", "stopped", "external"]);
    }

    #[test]
    fn pid_and_error_are_carried_through() {
        let mut failed = info(
            "boom",
            PrimitiveKind::Sink,
            PrimitiveState::Failed,
            &[],
            &[],
        );
        failed.error = Some("exited with code 1".to_string());
        let mut running = info(
            "alive",
            PrimitiveKind::Source,
            PrimitiveState::Running,
            &[],
            &[],
        );
        running.pid = Some(9001);

        let payload = build_topology_payload(1, vec![failed, running]);

        assert_eq!(
            payload.primitives[1].error.as_deref(),
            Some("exited with code 1")
        );
        assert_eq!(payload.primitives[1].pid, None);
        assert_eq!(payload.primitives[2].pid, Some(9001));
        assert!(payload.primitives[2].error.is_none());
    }

    #[test]
    fn publishes_and_subscribes_are_preserved() {
        let payload = build_topology_payload(
            1,
            vec![info(
                "filter",
                PrimitiveKind::Handler,
                PrimitiveState::Running,
                &["timer.filtered"],
                &["timer.tick", "timer.other"],
            )],
        );

        assert_eq!(payload.primitives[1].publishes, vec!["timer.filtered"]);
        assert_eq!(
            payload.primitives[1].subscribes,
            vec!["timer.tick", "timer.other"]
        );
    }

    /// The Rust, Python and TypeScript SDKs all deserialize this exact shape,
    /// so the serialized field names are part of the wire contract.
    #[test]
    fn serialized_shape_matches_the_sdk_contract() {
        let payload = build_topology_payload(
            7,
            vec![info(
                "timer",
                PrimitiveKind::Source,
                PrimitiveState::Running,
                &["timer.tick"],
                &[],
            )],
        );

        let value = serde_json::to_value(&payload).unwrap_or_default();
        let Some(primitives) = value
            .get("primitives")
            .and_then(serde_json::Value::as_array)
        else {
            panic!("serialized payload has no `primitives` array: {value}");
        };
        assert_eq!(primitives.len(), 2);

        let timer = &primitives[1];
        assert_eq!(timer.get("name").and_then(|v| v.as_str()), Some("timer"));
        assert_eq!(timer.get("kind").and_then(|v| v.as_str()), Some("source"));
        assert_eq!(timer.get("state").and_then(|v| v.as_str()), Some("running"));
        assert!(timer.get("publishes").is_some());
        assert!(timer.get("subscribes").is_some());
        assert!(timer.get("pid").is_some());
        assert!(timer.get("error").is_some());
    }
}
