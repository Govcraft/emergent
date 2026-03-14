package emergent

import "context"

// EmergentSink is a subscribe-only primitive (egress).
// Sinks consume events from the Emergent system but cannot publish messages.
type EmergentSink struct {
	*baseClient
}

// ConnectSink connects to the Emergent engine as a Sink.
func ConnectSink(name string, opts *ConnectOptions) (*EmergentSink, error) {
	client := newBaseClient(name, PrimitiveKindSink, opts)
	if err := client.connect(opts); err != nil {
		return nil, err
	}
	return &EmergentSink{baseClient: client}, nil
}

// Subscribe subscribes to the given message types and returns a MessageStream.
// The SDK automatically subscribes to system.shutdown and handles it internally.
func (s *EmergentSink) Subscribe(ctx context.Context, messageTypes []string) (*MessageStream, error) {
	return s.subscribeInternal(ctx, messageTypes)
}

// Unsubscribe unsubscribes from the given message types.
func (s *EmergentSink) Unsubscribe(ctx context.Context, messageTypes []string) error {
	return s.unsubscribeInternal(ctx, messageTypes)
}

// Discover queries the engine for available message types and primitives.
func (s *EmergentSink) Discover(ctx context.Context) (*DiscoveryInfo, error) {
	return s.discoverInternal(ctx)
}

// GetMySubscriptions queries the engine for the configured subscription types for this primitive.
func (s *EmergentSink) GetMySubscriptions(ctx context.Context) ([]string, error) {
	return s.getMySubscriptionsInternal(ctx)
}

// GetTopology queries the engine for the current topology of all primitives.
func (s *EmergentSink) GetTopology(ctx context.Context) (*TopologyState, error) {
	return s.getTopologyInternal(ctx)
}

// SubscribedTypes returns the list of currently subscribed message types.
func (s *EmergentSink) SubscribedTypes() []string {
	return s.subscribedTypesList()
}

// Name returns the sink's name.
func (s *EmergentSink) Name() string {
	return s.name
}

// Close gracefully disconnects from the engine.
func (s *EmergentSink) Close() error {
	return s.close()
}
