package emergent

import (
	"context"
	"time"
)

// EmergentHandler is a subscribe-and-publish primitive (transformation).
// Handlers subscribe to incoming messages, process them, and publish results.
type EmergentHandler struct {
	*baseClient
}

// ConnectHandler connects to the Emergent engine as a Handler.
func ConnectHandler(name string, opts *ConnectOptions) (*EmergentHandler, error) {
	client := newBaseClient(name, PrimitiveKindHandler, opts)
	if err := client.connect(opts); err != nil {
		return nil, err
	}
	return &EmergentHandler{baseClient: client}, nil
}

// Subscribe subscribes to the given message types and returns a MessageStream.
// The SDK automatically subscribes to system.shutdown and handles it internally.
func (h *EmergentHandler) Subscribe(ctx context.Context, messageTypes []string) (*MessageStream, error) {
	return h.subscribeInternal(ctx, messageTypes)
}

// Unsubscribe unsubscribes from the given message types.
func (h *EmergentHandler) Unsubscribe(ctx context.Context, messageTypes []string) error {
	return h.unsubscribeInternal(ctx, messageTypes)
}

// Publish publishes a message to the engine.
func (h *EmergentHandler) Publish(message *EmergentMessage) error {
	return h.publishInternal(message)
}

// PublishAck publishes a message and waits for broker acknowledgment.
// Unlike Publish, this provides backpressure by waiting for the engine's
// message broker to confirm it has processed and forwarded the message.
func (h *EmergentHandler) PublishAck(ctx context.Context, message *EmergentMessage) error {
	return h.publishInternalAck(ctx, message)
}

// PublishAll publishes all messages from a slice.
// Sends each message individually so subscribers begin consuming
// immediately. Stops on the first error.
// Returns the number of messages successfully published.
func (h *EmergentHandler) PublishAll(messages []*EmergentMessage) (int, error) {
	for i, msg := range messages {
		if err := h.PublishAck(context.Background(), msg); err != nil {
			return i, err
		}
	}
	return len(messages), nil
}

// PublishStream reads messages from a channel and publishes each one.
// Stops when the channel is closed, the context is cancelled, or
// a publish error occurs.
// Returns the number of messages successfully published.
func (h *EmergentHandler) PublishStream(ctx context.Context, ch <-chan *EmergentMessage) (int, error) {
	count := 0
	for {
		select {
		case <-ctx.Done():
			return count, ctx.Err()
		case msg, ok := <-ch:
			if !ok {
				return count, nil
			}
			if err := h.PublishAck(ctx, msg); err != nil {
				return count, err
			}
			count++
		}
	}
}

// StreamOffer offers items as a pull-based stream with consumer-driven backpressure.
// Publishes stream.ready, then serves items one at a time as the consumer
// sends stream.pull requests. Publishes stream.end when exhausted.
// Returns the number of items published.
func (h *EmergentHandler) StreamOffer(ctx context.Context, messageType string, items []map[string]any, pullStream *MessageStream, timeout time.Duration) (int, error) {
	streamID := generateCorrelationID("strm")

	// Announce stream
	ready, err := NewMessage("stream.ready")
	if err != nil {
		return 0, err
	}
	ready.WithPayload(map[string]any{"stream_id": streamID, "message_type": messageType, "count": len(items)})
	if err := h.Publish(ready); err != nil {
		return 0, err
	}

	published := 0
	idx := 0

	for {
		// Wait for pull request with timeout
		var msg *EmergentMessage
		select {
		case <-ctx.Done():
			return published, ctx.Err()
		case <-time.After(timeout):
			return published, &TimeoutError{Msg: "stream_offer timed out waiting for pull", Dur: timeout}
		case m, ok := <-pullStream.C():
			if !ok {
				return published, &ConnectionError{Msg: "pull stream closed during stream_offer"}
			}
			msg = m
		}

		// Check if it's a pull for this stream
		payload, _ := msg.Payload.(map[string]any)
		isPull := string(msg.MessageType) == "stream.pull" && payload != nil && payload["stream_id"] == streamID

		if isPull {
			if idx < len(items) {
				item, err := NewMessage(messageType)
				if err != nil {
					return published, err
				}
				item.WithPayload(items[idx]).WithMetadata(map[string]any{"stream_id": streamID})
				if err := h.Publish(item); err != nil {
					return published, err
				}
				published++
				idx++
			} else {
				end, err := NewMessage("stream.end")
				if err != nil {
					return published, err
				}
				end.WithPayload(map[string]any{"stream_id": streamID})
				if err := h.Publish(end); err != nil {
					return published, err
				}
				break
			}
		}
	}

	return published, nil
}

// StreamConsume consumes a pull-based stream, calling onItem for each item.
// Waits for stream.ready, then sends stream.pull requests automatically
// after each item is consumed. Stops on stream.end.
// Returns the number of items consumed.
func (h *EmergentHandler) StreamConsume(ctx context.Context, messageType string, sourceStream *MessageStream, timeout time.Duration, onItem func(*EmergentMessage)) (int, error) {
	// Wait for stream.ready
	var streamID string
	for {
		var msg *EmergentMessage
		select {
		case <-ctx.Done():
			return 0, ctx.Err()
		case <-time.After(timeout):
			return 0, &TimeoutError{Msg: "stream_consume timed out waiting for stream.ready", Dur: timeout}
		case m, ok := <-sourceStream.C():
			if !ok {
				return 0, &ConnectionError{Msg: "source stream closed before stream.ready"}
			}
			msg = m
		}

		if string(msg.MessageType) == "stream.ready" {
			payload, _ := msg.Payload.(map[string]any)
			if payload != nil && payload["message_type"] == messageType {
				if sid, ok := payload["stream_id"].(string); ok {
					streamID = sid
					break
				}
			}
		}
	}

	// Send initial pull
	pull, err := NewMessage("stream.pull")
	if err != nil {
		return 0, err
	}
	pull.WithPayload(map[string]any{"stream_id": streamID})
	if err := h.Publish(pull); err != nil {
		return 0, err
	}

	count := 0

	for {
		var msg *EmergentMessage
		select {
		case <-ctx.Done():
			return count, ctx.Err()
		case <-time.After(timeout):
			return count, &TimeoutError{Msg: "stream_consume timed out waiting for item", Dur: timeout}
		case m, ok := <-sourceStream.C():
			if !ok {
				return count, &ConnectionError{Msg: "source stream closed during stream_consume"}
			}
			msg = m
		}

		// Check for stream.end
		if string(msg.MessageType) == "stream.end" {
			payload, _ := msg.Payload.(map[string]any)
			if payload != nil && payload["stream_id"] == streamID {
				break
			}
		}

		// Check for matching data item
		meta, _ := msg.Metadata.(map[string]any)
		isItem := string(msg.MessageType) == messageType && meta != nil && meta["stream_id"] == streamID

		if isItem {
			onItem(msg)
			count++
			pull, err := NewMessage("stream.pull")
			if err != nil {
				return count, err
			}
			pull.WithPayload(map[string]any{"stream_id": streamID})
			if err := h.Publish(pull); err != nil {
				return count, err
			}
		}
	}

	return count, nil
}

// Discover queries the engine for available message types and primitives.
func (h *EmergentHandler) Discover(ctx context.Context) (*DiscoveryInfo, error) {
	return h.discoverInternal(ctx)
}

// SubscribedTypes returns the list of currently subscribed message types.
func (h *EmergentHandler) SubscribedTypes() []string {
	return h.subscribedTypesList()
}

// Name returns the handler's name.
func (h *EmergentHandler) Name() string {
	return h.name
}

// Close gracefully disconnects from the engine.
func (h *EmergentHandler) Close() error {
	return h.close()
}
