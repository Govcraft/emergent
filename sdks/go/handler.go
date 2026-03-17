package emergent

import "context"

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

// PublishAll publishes all messages from a slice.
// Sends each message individually so subscribers begin consuming
// immediately. Stops on the first error.
// Returns the number of messages successfully published.
func (h *EmergentHandler) PublishAll(messages []*EmergentMessage) (int, error) {
	for i, msg := range messages {
		if err := h.Publish(msg); err != nil {
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
			if err := h.Publish(msg); err != nil {
				return count, err
			}
			count++
		}
	}
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
