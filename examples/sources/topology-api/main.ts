#!/usr/bin/env -S deno run --allow-env --allow-read --allow-net
/**
 * Topology API Source - HTTP endpoint for topology refresh requests.
 *
 * Listens for HTTP requests and publishes system.request.topology messages.
 * The engine will respond with system.response.topology which sinks can consume.
 *
 * Usage:
 *   deno run --allow-env --allow-read --allow-net main.ts --port 8892
 *
 * Endpoints:
 *   GET /refresh - Triggers a topology refresh
 *   GET /health  - Health check
 */

import { EmergentSource, createMessage } from "../../../sdks/ts/mod.ts";
import { typeid } from "npm:typeid-js@1.2.0";

// Parse command line arguments
function parseArgs(): { port: number } {
  const args = Deno.args;
  let port = 8892;

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

async function main(): Promise<void> {
  const { port } = parseArgs();
  const name = Deno.env.get("EMERGENT_NAME") ?? "topology-api";

  console.log(`[${name}] Starting topology API source on port ${port}`);

  // Connect to engine as a source
  const source = await EmergentSource.connect(name);

  console.log(`[${name}] Connected to engine`);

  // Track request count for correlation
  let requestCount = 0;

  // Start HTTP server
  const server = Deno.serve({ port }, async (req: Request): Promise<Response> => {
    const url = new URL(req.url);
    const path = url.pathname;

    // CORS headers for browser requests
    const corsHeaders = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type",
    };

    // Handle CORS preflight
    if (req.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: corsHeaders });
    }

    switch (path) {
      case "/refresh": {
        if (req.method !== "GET" && req.method !== "POST") {
          return new Response("Method not allowed", {
            status: 405,
            headers: corsHeaders,
          });
        }

        requestCount++;
        const correlationId = typeid("topo").toString();

        console.log(`[${name}] Refresh request #${requestCount}`);

        // Publish topology request - engine will respond with system.response.topology
        await source.publish("system.request.topology", {
          correlationId,
          requestedBy: name,
        });

        return new Response(JSON.stringify({
          status: "ok",
          message: "Topology refresh requested",
          correlationId,
        }), {
          headers: {
            "Content-Type": "application/json",
            ...corsHeaders,
          },
        });
      }

      case "/health": {
        return new Response(JSON.stringify({ status: "healthy" }), {
          headers: {
            "Content-Type": "application/json",
            ...corsHeaders,
          },
        });
      }

      default:
        return new Response("Not Found", {
          status: 404,
          headers: corsHeaders,
        });
    }
  });

  console.log(`[${name}] HTTP server listening on http://localhost:${port}`);

  // Handle shutdown
  const shutdown = async () => {
    console.log(`[${name}] Shutting down`);
    await server.shutdown();
    source.close();
  };

  // Listen for SIGTERM
  Deno.addSignalListener("SIGTERM", shutdown);

  // Keep running until server closes
  await server.finished;
}

main();
