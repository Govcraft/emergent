package emergent

import (
	"context"
	"testing"
	"time"
)

// streamDeadline bounds every wait in these tests. The defects they cover are
// deadlocks, so a regression must fail the test instead of hanging it.
const streamDeadline = 3 * time.Second

// shutdownPush is the push notification the engine sends for a
// system.shutdown broadcast aimed at one primitive kind.
func shutdownPush(kind string) fakeReply {
	return fakeReply{
		msgType: MsgTypePush,
		payload: &IpcPushNotification{
			NotificationID: "ntf_shutdown",
			MessageType:    "system.shutdown",
			SourceActor:    "emergent",
			Payload:        engineShutdownEnvelope(kind),
		},
	}
}

// tickPush is the push notification for an ordinary timer.tick message.
func tickPush() fakeReply {
	return fakeReply{
		msgType: MsgTypePush,
		payload: &IpcPushNotification{
			NotificationID: "ntf_tick",
			MessageType:    "timer.tick",
			Payload: map[string]any{
				"id":           "msg_tick",
				"message_type": "timer.tick",
				"source":       "timer",
				"timestamp_ms": uint64(1758318000000),
				"payload":      map[string]any{"sequence": 1},
			},
		},
	}
}

// subscribedTestSink connects a sink and subscribes it to timer.tick. It
// registers no cleanup: each test closes the sink under a deadline itself,
// because a cleanup that deadlocks would hang the whole test binary.
func subscribedTestSink(t *testing.T, engine *fakeEngine) (*EmergentSink, *MessageStream) {
	t.Helper()
	sink, err := ConnectSink("shutdown_sink", &ConnectOptions{SocketPath: engine.socketPath, Timeout: 5 * time.Second})
	if err != nil {
		t.Fatalf("failed to connect sink: %v", err)
	}
	stream, err := sink.Subscribe(context.Background(), []string{"timer.tick"})
	if err != nil {
		t.Fatalf("failed to subscribe: %v", err)
	}
	return sink, stream
}

// awaitStreamClosed fails the test unless the stream's channel closes within
// the deadline.
func awaitStreamClosed(t *testing.T, stream *MessageStream) {
	t.Helper()
	deadline := time.After(streamDeadline)
	for {
		select {
		case _, open := <-stream.C():
			if !open {
				return
			}
		case <-deadline:
			t.Fatal("message stream did not close")
		}
	}
}

// closeWithin fails the test unless Close returns within the deadline.
func closeWithin(t *testing.T, sink *EmergentSink) {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- sink.Close() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("Close failed: %v", err)
		}
	case <-time.After(streamDeadline):
		t.Fatal("Close did not return: the read loop is holding c.mu")
	}
}

// TestMatchingShutdownClosesStream covers the system.shutdown path. The read
// loop handles the broadcast with c.mu held, and closing the stream there
// deadlocked on the stream's onClose callback, which takes c.mu again.
func TestMatchingShutdownClosesStream(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink, stream := subscribedTestSink(t, engine)

	engine.broadcast(shutdownPush("sink"))

	awaitStreamClosed(t, stream)
	closeWithin(t, sink)
}

// TestOtherKindShutdownLeavesStreamOpen checks that a broadcast aimed at
// another primitive kind is swallowed without ending the stream.
func TestOtherKindShutdownLeavesStreamOpen(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink, stream := subscribedTestSink(t, engine)

	engine.broadcast(shutdownPush("source"))
	engine.broadcast(shutdownPush("handler"))
	engine.broadcast(tickPush())

	select {
	case msg, open := <-stream.C():
		if !open {
			t.Fatal("stream closed on a shutdown aimed at another kind")
		}
		if msg.MessageType != "timer.tick" {
			t.Fatalf("expected timer.tick, got %s", msg.MessageType)
		}
	case <-time.After(streamDeadline):
		t.Fatal("timer.tick was not delivered after the other-kind shutdowns")
	}

	closeWithin(t, sink)
}

// TestEngineDisconnectClosesStream covers the EOF path: the engine side of
// the socket goes away, as it does when the engine is killed.
func TestEngineDisconnectClosesStream(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink, stream := subscribedTestSink(t, engine)

	engine.dropConnections()

	awaitStreamClosed(t, stream)
	closeWithin(t, sink)
}

// TestHandleShutdownDetachesWithoutClosing pins the contract the read loop
// relies on: with c.mu held, handleShutdown hands the stream back for the
// caller to close later and never closes it itself.
func TestHandleShutdownDetachesWithoutClosing(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	notification := func(kind string) map[string]any {
		return map[string]any{
			"message_type": "system.shutdown",
			"payload":      engineShutdownEnvelope(kind),
		}
	}

	tests := []struct {
		name         string
		notification map[string]any
		wantDetached bool
	}{
		{"matching kind", notification("sink"), true},
		{"matching kind in another case", notification("SINK"), true},
		{"other kind", notification("handler"), false},
		{"no kind", map[string]any{"message_type": "system.shutdown"}, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client := newBaseClient("unit_sink", PrimitiveKindSink, nil)
			stream := newMessageStream(1, nil)
			client.messageStream = stream

			client.mu.Lock()
			detached := client.handleShutdown(tt.notification)
			registered := client.messageStream
			client.mu.Unlock()

			if stream.IsClosed() {
				t.Fatal("handleShutdown closed the stream while c.mu was held")
			}
			if tt.wantDetached {
				if detached != stream || registered != nil {
					t.Fatalf("expected the stream to be detached, got detached=%p registered=%p", detached, registered)
				}
				return
			}
			if detached != nil || registered != stream {
				t.Fatalf("expected the stream to stay registered, got detached=%p registered=%p", detached, registered)
			}
		})
	}
}

// TestClosingAReplacedStreamKeepsTheNewOne checks the onClose callback only
// forgets its own stream. A detached stream is closed after c.mu is released,
// and a second Subscribe can register a new stream in that window.
func TestClosingAReplacedStreamKeepsTheNewOne(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink, first := subscribedTestSink(t, engine)

	second, err := sink.Subscribe(context.Background(), []string{"timer.tick"})
	if err != nil {
		t.Fatalf("failed to subscribe again: %v", err)
	}
	first.Close()

	sink.mu.Lock()
	registered := sink.messageStream
	sink.mu.Unlock()
	if registered != second {
		t.Fatal("closing the replaced stream unregistered the new one")
	}

	closeWithin(t, sink)
}
