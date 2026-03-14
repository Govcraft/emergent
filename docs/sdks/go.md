# Go SDK

The `emergent` package provides the Go SDK for building Sources, Handlers, and Sinks.

## Installation

```bash
go get github.com/govcraft/emergent/sdks/go
```

## Core Types

### EmergentMessage

The universal message envelope:

```go
import emergent "github.com/govcraft/emergent/sdks/go"

// Create a message
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
// Get raw payload
raw := msg.Payload

// Deserialize to typed struct
type MyPayload struct {
    Count int    `json:"count"`
    Name  string `json:"name"`
}

var payload MyPayload
err := msg.PayloadAs(&payload)
```

## EmergentSource

Sources publish messages into the system.

```go
import (
    "context"
    "time"

    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSource("my_source", func(ctx context.Context, source *emergent.EmergentSource) error {
        ticker := time.NewTicker(time.Second)
        defer ticker.Stop()
        var count int64

        for {
            select {
            case <-ctx.Done():
                return nil
            case <-ticker.C:
                count++
                msg, _ := emergent.NewMessage("counter.tick")
                msg.WithPayload(map[string]any{"count": count})
                source.Publish(msg)
            }
        }
    })
}
```

### Source API

| Method | Description |
|--------|-------------|
| `ConnectSource(name, opts)` | Connect to engine |
| `Publish(message)` | Send a message (fire-and-forget) |
| `Discover(ctx)` | Query available message types and primitives |
| `Close()` | Graceful disconnection |
| `Name()` | Get this source's name |

## EmergentHandler

Handlers subscribe to messages and publish new ones.

```go
import emergent "github.com/govcraft/emergent/sdks/go"

func main() {
    emergent.RunHandler("my_handler", []string{"input.event"}, func(msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
        var input struct {
            Value int64 `json:"value"`
        }
        msg.PayloadAs(&input)

        result, _ := emergent.NewMessage("output.event")
        result.WithCausationFromMessage(msg.ID).
            WithPayload(map[string]any{"doubled": input.Value * 2})

        return handler.Publish(result)
    })
}
```

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

## EmergentSink

Sinks subscribe to messages for output.

```go
import (
    "fmt"

    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSink("my_sink", []string{"counter.tick"}, func(msg *emergent.EmergentMessage) error {
        fmt.Printf("[%s] %s: %v\n", msg.MessageType, msg.Source, msg.Payload)
        return nil
    })
}
```

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

## Helper Functions

Helpers eliminate boilerplate for the common case:

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

All helpers handle connection, signal handling (SIGTERM/SIGINT), graceful shutdown, and cleanup automatically.

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
| `SocketNotFoundError` | Engine socket doesn't exist |
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

// Use ctx for cancellation
<-ctx.Done()
```

For Handlers and Sinks, the SDK automatically handles `system.shutdown` from the engine—the message stream closes and the `for range stream.C()` loop exits naturally.

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
