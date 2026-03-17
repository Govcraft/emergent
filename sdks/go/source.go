package emergent

import "context"

// EmergentSource is a publish-only primitive (ingress).
// Sources emit events into the Emergent system but cannot subscribe to messages.
type EmergentSource struct {
	*baseClient
}

// ConnectSource connects to the Emergent engine as a Source.
func ConnectSource(name string, opts *ConnectOptions) (*EmergentSource, error) {
	client := newBaseClient(name, PrimitiveKindSource, opts)
	if err := client.connect(opts); err != nil {
		return nil, err
	}
	return &EmergentSource{baseClient: client}, nil
}

// Publish publishes a message to the engine (fire-and-forget).
func (s *EmergentSource) Publish(message *EmergentMessage) error {
	return s.publishInternal(message)
}

// PublishAck publishes a message and waits for broker acknowledgment.
// Unlike Publish, this provides backpressure by waiting for the engine's
// message broker to confirm it has processed and forwarded the message.
func (s *EmergentSource) PublishAck(ctx context.Context, message *EmergentMessage) error {
	return s.publishInternalAck(ctx, message)
}

// PublishAll publishes all messages from a slice.
// Sends each message individually so subscribers begin consuming
// immediately. Stops on the first error.
// Returns the number of messages successfully published.
func (s *EmergentSource) PublishAll(messages []*EmergentMessage) (int, error) {
	for i, msg := range messages {
		if err := s.PublishAck(context.Background(), msg); err != nil {
			return i, err
		}
	}
	return len(messages), nil
}

// PublishStream reads messages from a channel and publishes each one.
// Stops when the channel is closed, the context is cancelled, or
// a publish error occurs.
// Returns the number of messages successfully published.
func (s *EmergentSource) PublishStream(ctx context.Context, ch <-chan *EmergentMessage) (int, error) {
	count := 0
	for {
		select {
		case <-ctx.Done():
			return count, ctx.Err()
		case msg, ok := <-ch:
			if !ok {
				return count, nil
			}
			if err := s.PublishAck(ctx, msg); err != nil {
				return count, err
			}
			count++
		}
	}
}

// Discover queries the engine for available message types and primitives.
func (s *EmergentSource) Discover(ctx context.Context) (*DiscoveryInfo, error) {
	return s.discoverInternal(ctx)
}

// Name returns the source's name.
func (s *EmergentSource) Name() string {
	return s.name
}

// Close gracefully disconnects from the engine.
func (s *EmergentSource) Close() error {
	return s.close()
}
