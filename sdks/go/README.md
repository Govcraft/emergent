# emergent

Go SDK for building event-driven workflows on the
[Emergent](https://github.com/govcraft/emergent) engine. Connect to a running
engine over Unix IPC and publish or consume messages through three typed
primitives: **Source**, **Handler**, and **Sink**.

```go
emergent.RunSink("my_sink", []string{"timer.tick"}, func(msg *emergent.EmergentMessage) error {
    fmt.Println(msg.Payload)
    return nil
})
```

## Install

```bash
go get github.com/govcraft/emergent/sdks/go
```

Then import:

```go
import emergent "github.com/govcraft/emergent/sdks/go"
```

## Three Primitives

Every Emergent workflow is composed of Sources, Handlers, and Sinks. Each
primitive has a single, well-defined role:

| Primitive     | Subscribe | Publish | Role                                        |
| ------------- | --------- | ------- | ------------------------------------------- |
| **Source**    | --        | Yes     | Ingress -- bring data into the system       |
| **Handler**   | Yes       | Yes     | Processing -- transform, enrich, or route   |
| **Sink**      | Yes       | --      | Egress -- persist, display, or forward data |

## Quick Start

### Sink -- consume messages

A Sink subscribes to message types and processes each one as it arrives:

```go
emergent.RunSink("my_sink", []string{"timer.tick"}, func(msg *emergent.EmergentMessage) error {
    var data struct {
        Sequence int64 `json:"sequence"`
    }
    msg.PayloadAs(&data)
    fmt.Printf("Tick #%d\n", data.Sequence)
    return nil
})
```

For explicit lifecycle control, connect and subscribe separately:

```go
sink, _ := emergent.ConnectSink("my_sink", nil)
defer sink.Close()

stream, _ := sink.Subscribe(ctx, []string{"timer.tick", "timer.filtered"})
for msg := range stream.C() {
    fmt.Println(msg.MessageType, msg.Payload)
}
```

### Source -- publish messages

A Source publishes messages into the engine. It cannot subscribe:

```go
emergent.RunSource("my_source", func(ctx context.Context, source *emergent.EmergentSource) error {
    msg, _ := emergent.NewMessage("sensor.reading")
    msg.WithPayload(map[string]any{"value": 42.5, "unit": "celsius"})
    return source.Publish(msg)
})
```

### Handler -- subscribe and publish

A Handler subscribes to incoming messages and publishes new ones. Use
`WithCausationFromMessage` to link output messages to the input that triggered
them:

```go
emergent.RunHandler("order_processor", []string{"order.created"}, func(msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
    out, _ := emergent.NewMessage("order.processed")
    out.WithCausationFromMessage(msg.ID).
        WithPayload(map[string]any{"status": "ok"})
    return handler.Publish(out)
})
```

## Streaming Publish

Publish a batch or channel of messages. Each message is sent individually so
subscribers begin consuming immediately. Both methods return the count of
successfully published messages and stop on the first error.

```go
// From a slice
messages := make([]*emergent.EmergentMessage, len(records))
for i, r := range records {
    msg, _ := emergent.NewMessage("record.imported")
    msg.WithPayload(r)
    messages[i] = msg
}
count, err := source.PublishAll(messages)

// From a channel
ch := make(chan *emergent.EmergentMessage, 32)
go func() {
    defer close(ch)
    for i := 0; i < 100; i++ {
        msg, _ := emergent.NewMessage("batch.item")
        msg.WithPayload(map[string]any{"index": i})
        ch <- msg
    }
}()
count, err := source.PublishStream(ctx, ch)
```

Both `PublishAll` and `PublishStream` are available on `EmergentSource` and
`EmergentHandler`.

## Building Messages

`NewMessage` creates a message with a generated ID and timestamp. Chain `With*`
methods to set fields:

```go
msg, _ := emergent.NewMessage("sensor.reading")
msg.WithPayload(map[string]any{"value": 42.5, "unit": "celsius"}).
    WithMetadata(map[string]any{"sensor_id": "temp-01", "location": "room-a"})
```

Link messages into traceable chains with `WithCausationFromMessage` and
`WithCorrelationID`:

```go
reply, _ := emergent.NewMessage("order.confirmed")
reply.WithCausationFromMessage(originalMsg.ID).
    WithCorrelationID(requestID).
    WithPayload(map[string]any{"confirmed": true})
```

## Subscribing to Messages

`Subscribe` accepts a string slice and returns a channel-based `MessageStream`:

```go
stream, _ := sink.Subscribe(ctx, []string{"timer.tick", "timer.filtered"})
```

Iterate using `range` on the channel:

```go
for msg := range stream.C() {
    fmt.Println(msg.MessageType, msg.Payload)
}
```

Or use `Next()` for blocking receive and `TryNext()` for non-blocking:

```go
msg := stream.Next()     // blocks, returns nil on close
msg := stream.TryNext()  // non-blocking, returns nil if empty
```

### Typed payloads

`PayloadAs` unmarshals the payload into any Go struct via JSON:

```go
type SensorReading struct {
    Value float64 `json:"value"`
    Unit  string  `json:"unit"`
}

var reading SensorReading
err := msg.PayloadAs(&reading)
fmt.Printf("%.1f %s\n", reading.Value, reading.Unit)
```

## Resource Cleanup

All primitives implement `Close()`. Use `defer` for cleanup:

```go
sink, _ := emergent.ConnectSink("my_sink", nil)
defer sink.Close()
```

The SDK subscribes to `system.shutdown` internally. When the Emergent engine
signals a graceful shutdown, active message streams close automatically and
`range stream.C()` loops exit naturally.

## Helper Functions

`RunSource`, `RunHandler`, and `RunSink` eliminate connection and
signal-handling boilerplate. Each helper connects, sets up SIGTERM/SIGINT
handlers, runs your callback, and disconnects on completion:

```go
// Source -- ctx is cancelled on SIGTERM/SIGINT
emergent.RunSource("my_timer", func(ctx context.Context, source *emergent.EmergentSource) error {
    ticker := time.NewTicker(3 * time.Second)
    defer ticker.Stop()
    var count int64
    for {
        select {
        case <-ctx.Done():
            return nil
        case <-ticker.C:
            count++
            msg, _ := emergent.NewMessage("timer.tick")
            msg.WithPayload(map[string]any{"count": count})
            source.Publish(msg)
        }
    }
})

// Handler -- called once per message
emergent.RunHandler("my_handler", []string{"raw.event"}, func(msg *emergent.EmergentMessage, h *emergent.EmergentHandler) error {
    out, _ := emergent.NewMessage("processed")
    out.WithCausationFromMessage(msg.ID).WithPayload(map[string]any{"done": true})
    return h.Publish(out)
})

// Sink -- called once per message
emergent.RunSink("my_sink", []string{"timer.tick"}, func(msg *emergent.EmergentMessage) error {
    fmt.Println(msg.Payload)
    return nil
})
```

The name argument falls back to the `EMERGENT_NAME` environment variable when
set to an empty string.

## Error Handling

All SDK errors implement the `EmergentError` interface with a `Code()` method
for machine-readable classification. Use type switches for precise control:

```go
err := source.Publish(msg)

switch e := err.(type) {
case *emergent.SocketNotFoundError:
    fmt.Printf("Engine not running at: %s\n", e.Path)
case *emergent.TimeoutError:
    fmt.Printf("Timed out after %s\n", e.Dur)
case *emergent.ConnectionError:
    fmt.Printf("Connection failed: %s\n", e.Msg)
}
```

### Error Types

| Error                 | Code                  | Extra Fields     |
| --------------------- | --------------------- | ---------------- |
| `ConnectionError`     | `CONNECTION_FAILED`   | `Msg`            |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND`    | `Path`           |
| `TimeoutError`        | `TIMEOUT`             | `Msg`, `Dur`     |
| `ProtocolError`       | `PROTOCOL_ERROR`      | `Msg`            |
| `SubscriptionError`   | `SUBSCRIPTION_FAILED` | `Msg`, `MessageTypes` |
| `PublishError`        | `PUBLISH_FAILED`      | `Msg`, `MessageType`  |
| `DiscoveryError`      | `DISCOVERY_FAILED`    | `Msg`            |
| `DisposedError`       | `DISPOSED`            | `ClientType`     |
| `ValidationError`     | `VALIDATION_ERROR`    | `Msg`, `Field`   |

## Message Shape

Every message flowing through Emergent follows the same envelope:

| Field            | Type            | Description                          |
| ---------------- | --------------- | ------------------------------------ |
| `ID`             | `MessageId`     | Unique TypeID (`msg_<uuidv7>`)       |
| `MessageType`    | `MessageType`   | Routing key (e.g., `"timer.tick"`)   |
| `Source`         | `PrimitiveName` | Name of the publishing primitive     |
| `CorrelationID`  | `string`        | Links related messages               |
| `CausationID`    | `string`        | ID of the triggering message         |
| `TimestampMs`    | `Timestamp`     | Creation time (Unix ms)              |
| `Payload`        | `any`           | User-defined data                    |
| `Metadata`       | `any`           | Optional tracing/debug data          |

Use `msg.PayloadAs(&target)` to unmarshal the payload into a typed struct.

## System Events

The Emergent engine broadcasts lifecycle events that your primitives can
subscribe to:

| Event Pattern              | Payload Type              | Fired When              |
| -------------------------- | ------------------------- | ----------------------- |
| `system.started.<name>`    | `SystemEventPayload`      | Primitive started       |
| `system.stopped.<name>`    | `SystemEventPayload`      | Primitive stopped       |
| `system.error.<name>`      | `SystemEventPayload`      | Primitive failed        |
| `system.shutdown`          | `SystemShutdownPayload`   | Engine shutting down    |

Use the typed payload structs for safe access:

```go
if strings.HasPrefix(string(msg.MessageType), "system.started.") {
    var event emergent.SystemEventPayload
    msg.PayloadAs(&event)
    fmt.Printf("%s (%s) started with PID %d\n", event.Name, event.Kind, *event.PID)
}

if strings.HasPrefix(string(msg.MessageType), "system.error.") {
    var event emergent.SystemEventPayload
    msg.PayloadAs(&event)
    if event.IsError() {
        fmt.Printf("%s failed: %s\n", event.Name, *event.Error)
    }
}
```

## Requirements

- Go 1.23 or later
- A running Emergent engine with the `EMERGENT_SOCKET` environment variable set

## License

MIT
