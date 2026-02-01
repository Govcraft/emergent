/**
 * TypeScript interfaces for the Topology Viewer.
 * @module
 */

/** Status of a primitive node. */
export type NodeStatus = "running" | "stopped" | "error";

/** Kind of primitive in the Emergent system. */
export type PrimitiveKind = "source" | "handler" | "sink";

/** A node in the topology graph representing a primitive. */
export interface TopologyNode {
  /** Unique identifier (primitive name). */
  id: string;
  /** Type of primitive. */
  kind: PrimitiveKind;
  /** Current status. */
  status: NodeStatus;
  /** Message types this primitive publishes. */
  publishes: string[];
  /** Message types this primitive subscribes to. */
  subscribes: string[];
  /** Process ID if running. */
  pid?: number;
  /** Error message if in error state. */
  error?: string;
}

/** An edge in the topology graph representing a message flow. */
export interface TopologyEdge {
  /** Source node ID (publisher). */
  source: string;
  /** Target node ID (subscriber). */
  target: string;
  /** Message type flowing through this edge. */
  messageType: string;
}

/** Complete topology state. */
export interface TopologyState {
  /** All nodes in the topology. */
  nodes: TopologyNode[];
  /** All edges in the topology. */
  edges: TopologyEdge[];
}

/** Server-sent event message wrapper. */
export interface SSEMessage<T = unknown> {
  /** Event type identifier. */
  type: string;
  /** Event payload. */
  data: T;
  /** Unix timestamp in milliseconds. */
  timestamp: number;
}
