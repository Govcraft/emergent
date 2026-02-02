#!/usr/bin/env -S deno run --allow-env --allow-read --allow-net
/**
 * Topology Viewer Sink - Real-time workflow visualization.
 *
 * Subscribes to system lifecycle events and serves a D3.js-based
 * force-directed graph visualization via HTTP/SSE.
 *
 * Usage:
 *   deno run --allow-env --allow-read --allow-net main.ts --port 8080
 *
 * The SDK automatically handles system.shutdown for graceful shutdown.
 */

import { EmergentSink } from "../../../sdks/ts/mod.ts";
import type { SystemEventPayload, TopologyPrimitive } from "../../../sdks/ts/mod.ts";
import { TopologyGraph } from "./graph.ts";

// Parse command line arguments
function parseArgs(): { port: number } {
  const args = Deno.args;
  let port = 8080;

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--port" && args[i + 1]) {
      port = parseInt(args[i + 1], 10);
      if (isNaN(port) || port < 1 || port > 65535) {
        console.error("Invalid port number");
        Deno.exit(1);
      }
    }
  }

  return { port };
}

// Get current script directory for static file serving
const scriptDir = new URL(".", import.meta.url).pathname;

// Content types for static files
const contentTypes: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
};

// Read a static file
async function readStaticFile(filename: string): Promise<Response> {
  const ext = filename.substring(filename.lastIndexOf("."));
  const contentType = contentTypes[ext] ?? "application/octet-stream";

  try {
    const content = await Deno.readTextFile(`${scriptDir}static/${filename}`);
    return new Response(content, {
      headers: { "Content-Type": contentType },
    });
  } catch {
    return new Response("Not Found", { status: 404 });
  }
}

// Create SSE response
function createSSEStream(graph: TopologyGraph): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      graph.registerSSEClient(controller);
    },
    cancel() {
      // Controller cleanup happens in graph.broadcast when write fails
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
      "Access-Control-Allow-Origin": "*",
    },
  });
}

// Delay helper
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Subscriptions for system lifecycle events
const SUBSCRIPTIONS = [
  "system.started.*",
  "system.stopped.*",
  "system.error.*",
];

// Connect to engine with retry logic
async function connectWithRetry(
  name: string,
  graph: TopologyGraph,
  maxRetries = 30,
  retryDelayMs = 2000
): Promise<void> {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      console.log(`[${name}] Connecting to engine (attempt ${attempt}/${maxRetries})...`);

      // Connect explicitly and subscribe with our own types
      // (not relying on engine config since we're externally managed)
      const sink = await EmergentSink.connect(name);

      try {
        // Try to query current topology to populate initial state
        // This may fail if the GetTopology IPC routing isn't working
        try {
          console.log(`[${name}] Connected, querying current topology...`);
          const topology = await sink.getTopology();
          console.log(`[${name}] Got ${topology.primitives.length} primitive(s)`);

          // Populate graph with existing primitives
          for (const prim of topology.primitives) {
            graph.handleTopologyPrimitive(prim as TopologyPrimitive);
          }
        } catch (topoErr) {
          console.log(`[${name}] Could not query topology (will rely on events): ${topoErr instanceof Error ? topoErr.message : topoErr}`);
        }

        // Subscribe to real-time updates
        console.log(`[${name}] Subscribing to: ${SUBSCRIPTIONS.join(", ")}`);
        const stream = await sink.subscribe(SUBSCRIPTIONS);

        try {
          for await (const msg of stream) {
            const payload = msg.payloadAs<SystemEventPayload>();

            if (msg.messageType.startsWith("system.started.")) {
              console.log(
                `[${name}] Started: ${payload.name} (${payload.kind}) pid=${payload.pid}`
              );
              graph.handleStarted(payload);
            } else if (msg.messageType.startsWith("system.stopped.")) {
              console.log(`[${name}] Stopped: ${payload.name}`);
              graph.handleStopped(payload);
            } else if (msg.messageType.startsWith("system.error.")) {
              console.log(`[${name}] Error: ${payload.name} - ${payload.error}`);
              graph.handleError(payload);
            }
          }
        } finally {
          stream.close();
        }
      } finally {
        sink.close();
      }

      // Stream ended gracefully (shutdown)
      console.log(`[${name}] Engine connection closed`);
      return;
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      console.log(`[${name}] Connection failed: ${errorMsg}`);

      if (attempt < maxRetries) {
        console.log(`[${name}] Retrying in ${retryDelayMs / 1000}s...`);
        await delay(retryDelayMs);
      }
    }
  }

  console.error(`[${name}] Failed to connect after ${maxRetries} attempts`);
}

// Main entry point
async function main(): Promise<void> {
  const { port } = parseArgs();
  const name = Deno.env.get("EMERGENT_NAME") ?? "topology-viewer";
  const graph = new TopologyGraph();

  console.log(`[${name}] Starting topology viewer on port ${port}`);

  // Start HTTP server first (non-blocking)
  const server = Deno.serve({ port }, (req: Request): Response | Promise<Response> => {
    const url = new URL(req.url);
    const path = url.pathname;

    switch (path) {
      case "/":
        return readStaticFile("index.html");
      case "/app.js":
        return readStaticFile("app.js");
      case "/style.css":
        return readStaticFile("style.css");
      case "/events":
        return createSSEStream(graph);
      case "/api/topology":
        return new Response(JSON.stringify(graph.getFullState()), {
          headers: {
            "Content-Type": "application/json",
            "Access-Control-Allow-Origin": "*",
          },
        });
      default:
        return new Response("Not Found", { status: 404 });
    }
  });

  console.log(`[${name}] HTTP server listening on http://localhost:${port}`);

  // Connect to engine with retry (runs in background)
  await connectWithRetry(name, graph);

  console.log(`[${name}] Shutting down`);
  await server.shutdown();
}

main();
