package emergent

import (
	"context"
	"testing"
	"time"
)

// A client feeds one stream. A second Subscribe takes the registration, and
// after that nothing could ever end the first stream, so Subscribe ends it.
func TestSecondSubscribeEndsTheStreamItReplaces(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink, first := subscribedTestSink(t, engine)

	// A consumer ranging over the first stream, the way RunSink does.
	ranged := make(chan int, 1)
	go func() {
		count := 0
		for range first.C() {
			count++
		}
		ranged <- count
	}()

	second, err := sink.Subscribe(context.Background(), []string{"timer.other"})
	if err != nil {
		t.Fatalf("second subscribe failed: %v", err)
	}

	select {
	case count := <-ranged:
		if count != 0 {
			t.Errorf("the first stream delivered %d messages, want 0", count)
		}
	case <-time.After(streamDeadline):
		t.Error("the range over the first stream was still waiting after the second subscribe")
	}
	if !first.IsClosed() {
		t.Error("the first stream is still open")
	}
	if second.IsClosed() {
		t.Error("the second stream is closed")
	}

	// The engine keeps the first subscription, so its topic still arrives,
	// on the stream that is registered now.
	engine.broadcast(tickPush())
	select {
	case msg, open := <-second.C():
		if !open {
			t.Fatal("the second stream closed")
		}
		if msg.MessageType != "timer.tick" {
			t.Errorf("message type = %q, want timer.tick", msg.MessageType)
		}
	case <-time.After(streamDeadline):
		t.Error("the second stream never received the first subscription's topic")
	}

	closeWithin(t, sink)
	awaitStreamClosed(t, second)
}
