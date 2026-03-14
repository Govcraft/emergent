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
