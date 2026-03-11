/**
 * Topology graph state management.
 * @module
 */

import type {
  TopologyNode,
  TopologyEdge,
  TopologyState,
  SSEMessage,
  NodeStatus,
  PrimitiveKind,
} from "./types.ts";
import type { SystemEventPayload, TopologyPrimitive } from "../../../sdks/ts/mod.ts";

/**
 * Manages the topology graph state and SSE broadcasting.
 */
export class TopologyGraph {
  private nodes: Map<string, TopologyNode> = new Map();
  private sseClients: Set<ReadableStreamDefaultController<Uint8Array>> =
    new Set();
  private encoder = new TextEncoder();

  /**
   * Add an initial known node (e.g., the engine itself).
   * Used for nodes that don't generate system.started events.
   */
  addInitialNode(node: TopologyNode): void {
    if (!this.nodes.has(node.id)) {
      this.nodes.set(node.id, node);
    }
  }

  /**
   * Handle a system.started event.
   */
  handleStarted(payload: SystemEventPayload): void {
    const node: TopologyNode = {
      id: payload.name,
      kind: payload.kind as PrimitiveKind,
      status: "running",
      publishes: payload.publishes ? [...payload.publishes] : [],
      subscribes: payload.subscribes ? [...payload.subscribes] : [],
      pid: payload.pid,
    };

    const isNew = !this.nodes.has(payload.name);
    this.nodes.set(payload.name, node);

    if (isNew) {
      this.broadcast({
        type: "node:added",
        data: node,
        timestamp: Date.now(),
      });
    } else {
      this.broadcast({
        type: "node:updated",
        data: node,
        timestamp: Date.now(),
      });
    }

    this.broadcastEdges();
  }

  /**
   * Handle a system.stopped event.
   */
  handleStopped(payload: SystemEventPayload): void {
    const existing = this.nodes.get(payload.name);
    if (existing) {
      existing.status = "stopped";
      existing.pid = undefined;
      this.broadcast({
        type: "node:updated",
        data: existing,
        timestamp: Date.now(),
      });
      this.broadcastEdges();
    }
  }

  /**
   * Handle any message flowing through the system.
   * Updates node status and broadcasts activity.
   */
  handleMessage(source: string, messageType: string): void {
    // Update source node to running if it exists
    const sourceNode = this.nodes.get(source);
    if (sourceNode && sourceNode.status !== "running") {
      sourceNode.status = "running";
      this.broadcast({
        type: "node:updated",
        data: sourceNode,
        timestamp: Date.now(),
      });
    }

    // Broadcast message activity for edge animation
    this.broadcast({
      type: "message:activity",
      data: { source, messageType },
      timestamp: Date.now(),
    });
  }

  /**
   * Handle a system.error event.
   */
  handleError(payload: SystemEventPayload): void {
    const existing = this.nodes.get(payload.name);
    if (existing) {
      existing.status = "error";
      existing.error = payload.error;
      this.broadcast({
        type: "node:updated",
        data: existing,
        timestamp: Date.now(),
      });
    } else {
      const node: TopologyNode = {
        id: payload.name,
        kind: payload.kind as PrimitiveKind,
        status: "error",
        publishes: payload.publishes ? [...payload.publishes] : [],
        subscribes: payload.subscribes ? [...payload.subscribes] : [],
        pid: payload.pid,
        error: payload.error,
      };
      this.nodes.set(payload.name, node);
      this.broadcast({
        type: "node:added",
        data: node,
        timestamp: Date.now(),
      });
      this.broadcastEdges();
    }
  }

  /**
   * Handle a full topology refresh from system.response.topology.
   * Updates all nodes and broadcasts full state to SSE clients.
   */
  handleTopologyRefresh(primitives: ReadonlyArray<{
    name: string;
    kind: string;
    state: string;
    publishes: readonly string[];
    subscribes: readonly string[];
    pid?: number;
    error?: string;
  }>): void {
    // Clear existing nodes except manually added ones (like emergent-engine)
    // Actually, let's replace everything with the authoritative response
    this.nodes.clear();

    for (const prim of primitives) {
      this.handleTopologyPrimitive(prim as TopologyPrimitive);
    }

    // Broadcast full state to all SSE clients
    this.broadcast({
      type: "topology:full",
      data: this.getFullState(),
      timestamp: Date.now(),
    });
  }

  /**
   * Handle a primitive from the topology query (initial state).
   */
  handleTopologyPrimitive(prim: TopologyPrimitive): void {
    // Map state to our NodeStatus
    let status: NodeStatus;
    switch (prim.state) {
      case "running":
        status = "running";
        break;
      case "stopped":
      case "stopping":
      case "configured":
        status = "stopped";
        break;
      case "failed":
        status = "error";
        break;
      default:
        status = "stopped";
    }

    const node: TopologyNode = {
      id: prim.name,
      kind: prim.kind as PrimitiveKind,
      status,
      publishes: [...prim.publishes],
      subscribes: [...prim.subscribes],
      pid: prim.pid,
      error: prim.error,
    };

    const isNew = !this.nodes.has(prim.name);
    this.nodes.set(prim.name, node);

    if (isNew) {
      this.broadcast({
        type: "node:added",
        data: node,
        timestamp: Date.now(),
      });
    } else {
      this.broadcast({
        type: "node:updated",
        data: node,
        timestamp: Date.now(),
      });
    }

    this.broadcastEdges();
  }

  /**
   * Check if a message type matches a subscription pattern.
   * Supports wildcards:
   *   - "*" matches everything
   *   - "system.started.*" matches "system.started.timer"
   */
  private matchesPattern(messageType: string, pattern: string): boolean {
    if (pattern === "*") return true; // Catch-all wildcard
    if (pattern === messageType) return true;
    if (!pattern.includes("*")) return false;

    const regex = new RegExp(
      "^" + pattern.replace(/\./g, "\\.").replace(/\*/g, "[^.]+") + "$"
    );
    return regex.test(messageType);
  }

  /**
   * Compute edges based on publish/subscribe relationships.
   */
  computeEdges(): TopologyEdge[] {
    const edges: TopologyEdge[] = [];
    const publishers = Array.from(this.nodes.values()).filter(
      (n) => n.publishes.length > 0
    );
    const subscribers = Array.from(this.nodes.values()).filter(
      (n) => n.subscribes.length > 0
    );

    for (const pub of publishers) {
      for (const messageType of pub.publishes) {
        for (const sub of subscribers) {
          if (pub.id === sub.id) continue;
          for (const pattern of sub.subscribes) {
            if (this.matchesPattern(messageType, pattern)) {
              edges.push({
                source: pub.id,
                target: sub.id,
                messageType,
              });
              break;
            }
          }
        }
      }
    }

    return edges;
  }

  /**
   * Get the full topology state.
   */
  getFullState(): TopologyState {
    return {
      nodes: Array.from(this.nodes.values()),
      edges: this.computeEdges(),
    };
  }

  /**
   * Register an SSE client controller.
   */
  registerSSEClient(
    controller: ReadableStreamDefaultController<Uint8Array>
  ): void {
    this.sseClients.add(controller);

    // Send full state to new client
    const state = this.getFullState();
    const message = this.formatSSE({
      type: "topology:full",
      data: state,
      timestamp: Date.now(),
    });

    try {
      controller.enqueue(this.encoder.encode(message));
    } catch {
      this.sseClients.delete(controller);
    }
  }

  /**
   * Unregister an SSE client controller.
   */
  unregisterSSEClient(
    controller: ReadableStreamDefaultController<Uint8Array>
  ): void {
    this.sseClients.delete(controller);
  }

  /**
   * Format a message as SSE data.
   */
  private formatSSE<T>(event: SSEMessage<T>): string {
    return `event: ${event.type}\ndata: ${JSON.stringify(event.data)}\n\n`;
  }

  /**
   * Broadcast an event to all SSE clients.
   */
  broadcast<T>(event: SSEMessage<T>): void {
    const message = this.formatSSE(event);
    const data = this.encoder.encode(message);

    for (const controller of this.sseClients) {
      try {
        controller.enqueue(data);
      } catch {
        this.sseClients.delete(controller);
      }
    }
  }

  /**
   * Broadcast updated edges to all clients.
   */
  private broadcastEdges(): void {
    this.broadcast({
      type: "edges:updated",
      data: this.computeEdges(),
      timestamp: Date.now(),
    });
  }

  /**
   * Get the number of connected SSE clients.
   */
  get clientCount(): number {
    return this.sseClients.size;
  }

  /**
   * Get the number of nodes.
   */
  get nodeCount(): number {
    return this.nodes.size;
  }
}
