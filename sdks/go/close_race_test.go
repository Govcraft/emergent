package emergent

import (
	"context"
	"errors"
	"net"
	"sync"
	"testing"
	"time"
)

func TestUsableConn(t *testing.T) {
	client, server := net.Pipe()
	defer func() { _ = client.Close() }()
	defer func() { _ = server.Close() }()

	t.Run("connected", func(t *testing.T) {
		conn, err := usableConn(connState{conn: client, kind: PrimitiveKindSource})
		if err != nil || conn != client {
			t.Fatalf("usableConn() = (%v, %v), want the connection", conn, err)
		}
	})

	t.Run("never connected", func(t *testing.T) {
		conn, err := usableConn(connState{kind: PrimitiveKindSource})
		var connErr *ConnectionError
		if conn != nil || !errors.As(err, &connErr) {
			t.Fatalf("usableConn() = (%v, %v), want a ConnectionError", conn, err)
		}
	})

	t.Run("disposed wins over a live connection", func(t *testing.T) {
		conn, err := usableConn(connState{disposed: true, conn: client, kind: PrimitiveKindSink})
		var disposedErr *DisposedError
		if conn != nil || !errors.As(err, &disposedErr) {
			t.Fatalf("usableConn() = (%v, %v), want a DisposedError", conn, err)
		}
		if disposedErr.ClientType != string(PrimitiveKindSink) {
			t.Fatalf("ClientType = %q, want %q", disposedErr.ClientType, PrimitiveKindSink)
		}
	})
}

// isClosedClientError reports whether err is one a caller may see when Close
// wins the race against its call.
func isClosedClientError(err error) bool {
	var disposedErr *DisposedError
	var connErr *ConnectionError
	var publishErr *PublishError
	return errors.As(err, &disposedErr) || errors.As(err, &connErr) || errors.As(err, &publishErr)
}

// TestCloseRacingPublishAndRequests runs Close while publishes and lookups are
// in flight. close() sets c.conn to nil under c.mu, so `go test -race` fails
// here if a writer reads c.conn without that lock, and a nil connection would
// panic the writer.
func TestCloseRacingPublishAndRequests(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	const rounds = 20
	const publishers = 4

	for round := 0; round < rounds; round++ {
		engine := startFakeEngine(t, 0)
		handler, err := ConnectHandler("race_handler", &ConnectOptions{SocketPath: engine.socketPath, Timeout: 5 * time.Second})
		if err != nil {
			t.Fatalf("failed to connect handler: %v", err)
		}

		started := make(chan struct{}, publishers+1)
		errCh := make(chan error, publishers+1)
		var wg sync.WaitGroup

		for i := 0; i < publishers; i++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				for n := 0; ; n++ {
					msg, msgErr := NewMessage("race.tick")
					if msgErr != nil {
						errCh <- msgErr
						return
					}
					if n == 1 {
						started <- struct{}{}
					}
					if pubErr := handler.Publish(msg); pubErr != nil {
						errCh <- pubErr
						return
					}
				}
			}()
		}

		wg.Add(1)
		go func() {
			defer wg.Done()
			for n := 0; ; n++ {
				if n == 1 {
					started <- struct{}{}
				}
				if _, topoErr := handler.getTopologyInternal(context.Background()); topoErr != nil {
					errCh <- topoErr
					return
				}
			}
		}()

		// Close only once every worker has a call in flight.
		for i := 0; i < publishers+1; i++ {
			select {
			case <-started:
			case <-time.After(streamDeadline):
				t.Fatal("workers did not start")
			}
		}
		if closeErr := handler.Close(); closeErr != nil {
			t.Fatalf("Close failed: %v", closeErr)
		}

		wg.Wait()
		close(errCh)
		for workerErr := range errCh {
			if !isClosedClientError(workerErr) {
				t.Errorf("round %d: unexpected error after Close: %v", round, workerErr)
			}
		}
		assertNoPendingRequests(t, handler.baseClient)
		engine.stop()
	}
}
