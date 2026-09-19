package emergent

import (
	"sync"
	"testing"
	"time"
)

// TestPushRacingClose closes a stream while another goroutine pushes to it,
// which is what happens when user code calls Close during delivery. push must
// never send on the closed channel.
func TestPushRacingClose(t *testing.T) {
	for round := 0; round < 200; round++ {
		stream := newMessageStream(4, nil)
		msg := &EmergentMessage{MessageType: "race.tick"}

		var wg sync.WaitGroup
		wg.Add(2)
		go func() {
			defer wg.Done()
			for i := 0; i < 200; i++ {
				stream.push(msg)
			}
		}()
		go func() {
			defer wg.Done()
			stream.Close()
		}()
		wg.Wait()
	}
}

// TestUserCloseRacingDelivery closes the stream from the test goroutine while
// the read loop is delivering a flood of messages with c.mu held. Close used
// to run the onClose callback, which takes c.mu, while still holding the
// stream mutex that push was waiting for.
func TestUserCloseRacingDelivery(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	for round := 0; round < 25; round++ {
		engine := startFakeEngine(t, 0)
		sink, stream := subscribedTestSink(t, engine)

		flooding := make(chan struct{})
		var wg sync.WaitGroup
		wg.Add(1)
		go func() {
			defer wg.Done()
			close(flooding)
			for i := 0; i < 500; i++ {
				engine.broadcast(tickPush())
			}
		}()

		<-flooding
		closed := make(chan struct{})
		go func() {
			stream.Close()
			close(closed)
		}()
		select {
		case <-closed:
		case <-time.After(streamDeadline):
			t.Fatal("stream.Close did not return: it is deadlocked against the read loop")
		}
		closeWithin(t, sink)
		wg.Wait()
		engine.stop()
	}
}
