#!/usr/bin/env -S deno run --allow-env --allow-read --allow-net
/**
 * Topology Query Handler - Event-driven topology query handler.
 *
 * Subscribes to system.request.topology messages and queries the engine's
 * HTTP endpoint to get the topology, then publishes system.response.topology.
 *
 * This separates concerns:
 * - Engine is pure infrastructure (provides HTTP endpoint for topology)
 * - This handler does the work (queries engine, publishes response)
 *
 * Usage:
 *   deno run --allow-env --allow-read --allow-net main.ts
 */

import { EmergentHandler } from "../../../sdks/ts/mod.ts";

// Engine's HTTP API endpoint for topology queries
const ENGINE_TOPOLOGY_URL = "http://127.0.0.1:8891/api/topology";

interface TopologyPrimitive {
  name: string;
  kind: string;
  state: string;
  publishes: string[];
  subscribes: string[];
  pid?: number;
  error?: string;
}

interface TopologyResponse {
  primitives: TopologyPrimitive[];
}

interface TopologyRequestPayload {
  correlationId?: string;
  requestedBy?: string;
}

async function main(): Promise<void> {
  const name = Deno.env.get("EMERGENT_NAME") ?? "topology-query";

  console.log(`[${name}] Starting topology query handler`);

  // Connect to engine as a handler (can subscribe and publish)
  const handler = await EmergentHandler.connect(name);

  console.log(`[${name}] Connected to engine`);

  // Subscribe to topology request messages
  const stream = await handler.subscribe(["system.request.topology"]);
  console.log(`[${name}] Subscribed to system.request.topology`);

  // Track request count
  let requestCount = 0;

  // Process incoming topology requests
  for await (const msg of stream) {
    if (msg.messageType === "system.request.topology") {
      requestCount++;
      console.log(`[${name}] Processing topology request #${requestCount}`);

      // Get the correlation ID from the request payload or message
      const requestPayload = msg.payloadAs<TopologyRequestPayload>();
      const correlationId = requestPayload?.correlationId ?? msg.correlationId;

      try {
        // Query the engine's HTTP endpoint directly
        const response = await fetch(ENGINE_TOPOLOGY_URL);
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}: ${response.statusText}`);
        }

        const topology: TopologyResponse = await response.json();
        console.log(
          `[${name}] Got ${topology.primitives.length} primitive(s) from engine`
        );

        // Publish the topology response with matching correlation ID
        await handler.publish("system.response.topology", {
          primitives: topology.primitives,
          correlationId,
        });

        console.log(`[${name}] Published system.response.topology`);
      } catch (err) {
        console.error(`[${name}] Failed to query topology:`, err);

        // Publish error response
        await handler.publish("system.response.topology", {
          primitives: [],
          error: err instanceof Error ? err.message : String(err),
          correlationId,
        });
      }
    }
  }
}

// Handle shutdown
const shutdown = () => {
  console.log(`[topology-query] Received shutdown signal`);
  Deno.exit(0);
};

Deno.addSignalListener("SIGTERM", shutdown);

main();
