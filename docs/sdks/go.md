# Go SDK

The `emergent` package provides the Go SDK for building custom Sources, Handlers, and Sinks. Use this when marketplace exec primitives are not enough -- you need persistent state across messages, custom protocols, or high-performance processing.

For stateless transformations (jq, model calls, data extraction), use the marketplace exec primitives instead. See [Getting Started](../getting-started.md).

## Installation

```bash
go get github.com/govcraft/emergent/sdks/go
```

Then import:

```go
import emergent "github.com/govcraft/emergent/sdks/go"
```

## Quick Examples

### Sink (consume messages)

```go
emergent.RunSink("my_sink", []string{"timer.tick"}, func(msg *emergent.EmergentMessage) error {
    var data struct {
        Count int64 `json:"count"`
    }
    msg.PayloadAs(&data)
    fmt.Printf("Tick #%d\n", data.Count)
    return nil
})
```

### Handler (transform messages)

```go
emergent.RunHandler("my_handler", []string{"data.raw"}, func(msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
    var input struct {
        Value int64 `json:"value"`
    }
    msg.PayloadAs(&input)

    result, _ := emergent.NewMessage("data.processed")
    result.WithCausationFromMessage(msg.ID).
        WithPayload(map[string]any{"doubled": input.Value * 2})

    return handler.Publish(result)
})
```

### Source (publish messages)

```go
emergent.RunSource("my_source", func(ctx context.Context, source *emergent.EmergentSource) error {
    ticker := time.NewTicker(3 * time.Second)
    defer ticker.Stop()

    for {
        select {
        case <-ctx.Done():
            return nil
        case <-ticker.C:
            msg, _ := emergent.NewMessage("timer.tick")
            msg.WithPayload(map[string]any{"timestamp": time.Now().UnixMilli()})
            source.Publish(msg)
        }
    }
})
```

## Full Documentation

See the complete SDK README for the full API reference, message types, error handling, and configuration examples:

- [Go SDK README](../../sdks/go/README.md)

## Core Types

### EmergentMessage

The universal message envelope:

```go
msg, _ := emergent.NewMessage("event.type")
msg.WithPayload(map[string]any{"key": "value"}).
    WithSource("my_source")

// With causation tracking
response, _ := emergent.NewMessage("event.processed")
response.WithCausationFromMessage(originalMsg.ID).
    WithPayload(map[string]any{"result": "ok"})

// With metadata
msg, _ := emergent.NewMessage("event.type")
msg.WithPayload(map[string]any{"data": 42}).
    WithMetadata(map[string]any{"trace_id": "abc123"})

// With correlation ID
msg, _ := emergent.NewMessage("api.response")
msg.WithCorrelationID("request-123").
    WithPayload(map[string]any{"status": "ok"})
```

### Message Fields

| Field | Type | Description |
|-------|------|-------------|
| `ID` | `MessageId` | Unique ID (`msg_<UUIDv7>`) |
| `MessageType` | `MessageType` | Type for subscription matching |
| `Source` | `PrimitiveName` | Primitive that emitted this |
| `CorrelationID` | `string` | Groups related messages |
| `CausationID` | `string` | ID of triggering message |
| `TimestampMs` | `Timestamp` | Unix timestamp in milliseconds |
| `Payload` | `any` | Application data |
| `Metadata` | `any` | Optional routing/debug info |

### Payload Extraction

```go
var payload MyPayload
err := msg.PayloadAs(&payload)
```

## Helper Functions

Helpers handle connection, signal handling (SIGTERM/SIGINT), graceful shutdown, and cleanup automatically:

```go
// Source: ctx is cancelled on SIGTERM/SIGINT
emergent.RunSource("name", func(ctx context.Context, source *emergent.EmergentSource) error {
    // Your source logic
})

// Handler: called for each message
emergent.RunHandler("name", []string{"topic"}, func(msg *emergent.EmergentMessage, h *emergent.EmergentHandler) error {
    // Process msg, optionally h.Publish(...)
})

// Sink: called for each message
emergent.RunSink("name", []string{"topic"}, func(msg *emergent.EmergentMessage) error {
    // Consume msg
})
```

## API Reference

### Source API

| Method | Description |
|--------|-------------|
| `ConnectSource(name, opts)` | Connect to engine |
| `Publish(message)` | Send a message (fire-and-forget) |
| `Discover(ctx)` | Query available message types and primitives |
| `Close()` | Graceful disconnection |
| `Name()` | Get this source's name |

### Handler API

| Method | Description |
|--------|-------------|
| `ConnectHandler(name, opts)` | Connect to engine |
| `Subscribe(ctx, types)` | Subscribe and get message stream |
| `Publish(message)` | Send a message |
| `Unsubscribe(ctx, types)` | Remove subscriptions |
| `Discover(ctx)` | Query available message types |
| `Close()` | Graceful disconnection |
| `SubscribedTypes()` | Get current subscriptions |

### Sink API

| Method | Description |
|--------|-------------|
| `ConnectSink(name, opts)` | Connect to engine |
| `Subscribe(ctx, types)` | Subscribe and get message stream |
| `GetMySubscriptions(ctx)` | Query configured subscriptions from engine |
| `GetTopology(ctx)` | Query current system topology |
| `Unsubscribe(ctx, types)` | Remove subscriptions |
| `Discover(ctx)` | Query available message types |
| `Close()` | Graceful disconnection |

## MessageStream

The subscription stream uses Go channels:

```go
stream, _ := handler.Subscribe(ctx, []string{"events"})

// Iterate with range (closes on shutdown)
for msg := range stream.C() {
    process(msg)
}

// Or use Next() for blocking receive
msg := stream.Next() // nil when closed

// Non-blocking receive
msg := stream.TryNext() // nil if nothing available
```

## Error Handling

```go
err := source.Publish(message)

switch e := err.(type) {
case *emergent.SocketNotFoundError:
    fmt.Fprintf(os.Stderr, "Socket not found: %s\n", e.Path)
case *emergent.ConnectionError:
    fmt.Fprintf(os.Stderr, "Connection failed: %s\n", e.Msg)
case *emergent.TimeoutError:
    fmt.Fprintf(os.Stderr, "Timed out: %s\n", e.Msg)
default:
    fmt.Fprintf(os.Stderr, "Error: %v\n", err)
}
```

### Error Types

| Error | Description |
|-------|-------------|
| `SocketNotFoundError` | Engine socket does not exist |
| `ConnectionError` | Failed to connect to engine |
| `TimeoutError` | Operation timed out |
| `SubscriptionError` | Subscription request failed |
| `DiscoveryError` | Discovery request failed |
| `ProtocolError` | Unexpected protocol message |
| `PublishError` | Message publish failed |
| `DisposedError` | Operation on closed client |
| `ValidationError` | Invalid message type or name |

## Graceful Shutdown

With helpers, shutdown is automatic. For manual control:

```go
source, _ := emergent.ConnectSource("my_source", nil)
defer source.Close()

ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
defer cancel()

<-ctx.Done()
```

For Handlers and Sinks, the SDK automatically handles `system.shutdown` from the engine -- the message stream closes and the `for range stream.C()` loop exits naturally.

## Configuration

Add Go primitives to `emergent.toml`:

```toml
[[sources]]
name = "timer-go"
path = "./timer-go"
args = ["--interval", "3000"]
enabled = true
publishes = ["timer.tick"]
```

## See Also

- [Rust SDK](rust.md) - crates.io: [emergent-client](https://crates.io/crates/emergent-client)
- [TypeScript SDK](typescript.md) - JSR: [@govcraft/emergent](https://jsr.io/@govcraft/emergent)
- [Python SDK](python.md) - PyPI: [emergent-client](https://pypi.org/project/emergent-client/)
- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
