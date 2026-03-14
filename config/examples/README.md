# Example Pipelines

Zero-code pipelines built entirely from marketplace primitives and TOML configuration. Each example runs a complete workflow without writing any application code.

## Prerequisites

Install the engine and the primitives used by these examples:

```bash
# Install all primitives used across examples
emergent marketplace install exec-source exec-handler exec-sink \
  http-source websocket-handler topology-viewer
```

## Examples

### basic-pipeline.toml

Runs `date` every 3 seconds, pipes through `jq` to add a field, pretty-prints the result. Includes the topology viewer on port 8009 for a live visualization of the dataflow.

```bash
emergent --config ./config/examples/basic-pipeline.toml
# Open http://localhost:8009 to see the topology
```

### ouroboros-loop.toml

An infinite loop. POST once to seed it, and the message circulates forever with an incrementing counter. Demonstrates how a sink can feed back into a source.

```bash
emergent --config ./config/examples/ouroboros-loop.toml

# Seed the loop from another terminal:
curl -X POST -H "Content-Type: application/json" \
  -d '{"count": 0, "message": "hello ouroboros"}' \
  http://localhost:8088
```

### websocket-echo.toml

Connects to a WebSocket echo server, sends a test message, prints the echoed response. Demonstrates the websocket-handler's bidirectional bridge.

```bash
emergent --config ./config/examples/websocket-echo.toml
```

### slack-bot.toml

A Claude-powered Slack chatbot using Socket Mode (no public URL needed). Receives messages via WebSocket, auto-acks envelopes within Slack's 3-second window, sends message text to Claude, and posts the response back to the channel.

Requires a Slack app configured with Socket Mode, `chat:write`, and message event subscriptions. See the comments in the TOML file for full setup steps.

```bash
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
emergent --config ./config/examples/slack-bot.toml
```

## Secrets

Pass secrets as environment variables. The engine forwards the parent process environment to all primitives, so `export` before running or use a `.env` file with `source .env`. Never hardcode tokens in TOML.
