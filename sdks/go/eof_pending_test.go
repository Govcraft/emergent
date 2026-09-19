package emergent

import (
	"context"
	"errors"
	"testing"
	"time"
)

// TestFailPendingSettlesEveryRequest pins the helper both close and the read
// loop's EOF path rely on: every in-flight request gets the error, its timer
// is stopped, and its entry is forgotten.
func TestFailPendingSettlesEveryRequest(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	client := newBaseClient("unit_sink", PrimitiveKindSink, nil)
	timer := func() *time.Timer { return time.NewTimer(time.Hour) }

	request := &pendingRequest{ch: make(chan ipcResponseResult, 1), timer: timer()}
	topology := &pendingPubSubRequest{ch: make(chan any, 1), timer: timer()}
	subscriptions := &pendingPubSubRequest{ch: make(chan any, 1), timer: timer()}
	// A request answered a moment ago keeps its answer.
	answered := &pendingPubSubRequest{ch: make(chan any, 1), timer: timer()}
	answered.ch <- "answer"

	client.pendingRequests["req"] = request
	client.pendingTopologyRequests["topo"] = topology
	client.pendingSubscriptionRequests["subs"] = subscriptions
	client.pendingSubscriptionRequests["answered"] = answered

	want := &ConnectionError{Msg: "connection closed"}
	client.mu.Lock()
	client.failPending(want)
	client.mu.Unlock()

	if got := (<-request.ch).err; got != error(want) {
		t.Errorf("request error = %v, want %v", got, want)
	}
	if got := <-topology.ch; got != any(want) {
		t.Errorf("topology result = %v, want %v", got, want)
	}
	if got := <-subscriptions.ch; got != any(want) {
		t.Errorf("subscriptions result = %v, want %v", got, want)
	}
	if got := <-answered.ch; got != "answer" {
		t.Errorf("answered result = %v, want the original answer", got)
	}

	pending := len(client.pendingRequests) + len(client.pendingTopologyRequests) + len(client.pendingSubscriptionRequests)
	if pending != 0 {
		t.Errorf("%d pending entries left, want 0", pending)
	}
	for name, tm := range map[string]*time.Timer{"request": request.timer, "topology": topology.timer} {
		if tm.Stop() {
			t.Errorf("%s timer was still running", name)
		}
	}
}

// TestEngineDisconnectFailsPendingLookups covers the engine dying while a
// lookup is in flight. The caller gets a ConnectionError at once instead of
// waiting out the request timeout (30 seconds here).
func TestEngineDisconnectFailsPendingLookups(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	lookups := []struct {
		name string
		call func(*EmergentSink) error
	}{
		{"GetTopology", func(sink *EmergentSink) error {
			_, err := sink.GetTopology(context.Background())
			return err
		}},
		{"GetMySubscriptions", func(sink *EmergentSink) error {
			_, err := sink.GetMySubscriptions(context.Background())
			return err
		}},
	}

	for _, lookup := range lookups {
		t.Run(lookup.name, func(t *testing.T) {
			engine := startFakeEngine(t, 0)
			engine.muteLookups.Store(true)
			sink := connectTestSink(t, engine, 30*time.Second)
			defer closeWithin(t, sink)

			result := make(chan error, 1)
			go func() { result <- lookup.call(sink) }()

			// Let the lookup get onto the wire before the engine goes away.
			time.Sleep(100 * time.Millisecond)
			engine.dropConnections()

			select {
			case err := <-result:
				var connErr *ConnectionError
				if !errors.As(err, &connErr) {
					t.Fatalf("error = %v, want a ConnectionError", err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("the lookup outlived the engine connection")
			}
		})
	}
}
