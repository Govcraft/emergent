package emergent

import (
	"context"
	"errors"
	"io"
	"net"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// silentEngine is the far end of a pipe that reads every frame and answers
// only the request types in `answers`, with success.
func silentEngine(t *testing.T, client *baseClient, server net.Conn, answers map[byte]bool) {
	t.Helper()
	go func() {
		var buffer []byte
		chunk := make([]byte, 4096)
		for {
			n, err := server.Read(chunk)
			if err != nil {
				return
			}
			buffer = append(buffer, chunk[:n]...)
			for {
				frame, decodeErr := TryDecodeFrame(buffer)
				if decodeErr != nil || frame == nil {
					break
				}
				buffer = buffer[frame.BytesConsumed:]
				if !answers[frame.MsgType] {
					continue
				}
				request, _ := frame.Payload.(map[string]any)
				reply, encodeErr := EncodeFrame(MsgTypeResponse, map[string]any{
					"correlation_id": request["correlation_id"],
					"success":        true,
				}, FormatMsgPack)
				if encodeErr != nil {
					return
				}
				feedFrames(client, reply)
			}
		}
	}()
}

// causeClient is a client on one end of a pipe whose requests time out fast.
func causeClient(t *testing.T, answers map[byte]bool) *baseClient {
	t.Helper()
	client, _ := unitClient(t)
	client.timeout = 50 * time.Millisecond
	conn, server := net.Pipe()
	t.Cleanup(func() {
		_ = conn.Close()
		_ = server.Close()
	})
	client.conn = conn
	// No read loop runs here, so close() has nothing to wait for.
	close(client.readDone)
	silentEngine(t, client, server, answers)
	return client
}

func causeMessage(t *testing.T) *EmergentMessage {
	t.Helper()
	msg, err := NewMessage("go73.event")
	if err != nil {
		t.Fatalf("NewMessage failed: %v", err)
	}
	return msg
}

// Each wrapping site keeps its cause, so errors.As and errors.Is reach the
// TimeoutError, ConnectionError, DisposedError or context error underneath.
func TestOperationErrorsKeepTheirCause(t *testing.T) {
	type operation struct {
		name string
		// answers lists the request types the engine replies to before the
		// one under test goes unanswered.
		answers map[byte]bool
		run     func(ctx context.Context, client *baseClient) error
		wrapper func(err error) (cause error, ok bool)
	}

	subscriptionCause := func(err error) (error, bool) {
		var wrapped *SubscriptionError
		if !errors.As(err, &wrapped) {
			return nil, false
		}
		return wrapped.Err, true
	}
	publishCause := func(err error) (error, bool) {
		var wrapped *PublishError
		if !errors.As(err, &wrapped) {
			return nil, false
		}
		return wrapped.Err, true
	}
	discoveryCause := func(err error) (error, bool) {
		var wrapped *DiscoveryError
		if !errors.As(err, &wrapped) {
			return nil, false
		}
		return wrapped.Err, true
	}

	operations := []operation{
		{
			name: "subscribe",
			run: func(ctx context.Context, client *baseClient) error {
				_, err := client.subscribeInternal(ctx, []string{"go73.event"})
				return err
			},
			wrapper: subscriptionCause,
		},
		{
			name:    "pattern subscribe",
			answers: map[byte]bool{MsgTypeSubscribe: true},
			run: func(ctx context.Context, client *baseClient) error {
				_, err := client.subscribeInternal(ctx, []string{"go73.*"})
				return err
			},
			wrapper: subscriptionCause,
		},
		{
			name: "acknowledged publish",
			run: func(ctx context.Context, client *baseClient) error {
				return client.publishInternalAck(ctx, causeMessage(t))
			},
			wrapper: publishCause,
		},
		{
			name: "discover",
			run: func(ctx context.Context, client *baseClient) error {
				_, err := client.discoverInternal(ctx)
				return err
			},
			wrapper: discoveryCause,
		},
	}

	for _, op := range operations {
		t.Run(op.name+"/timeout", func(t *testing.T) {
			client := causeClient(t, op.answers)
			err := op.run(context.Background(), client)

			cause, ok := op.wrapper(err)
			if !ok {
				t.Fatalf("error = %v (%T), want the operation's own error type", err, err)
			}
			var timeoutErr *TimeoutError
			if !errors.As(err, &timeoutErr) {
				t.Fatalf("errors.As(%v, *TimeoutError) = false, cause = %v", err, cause)
			}
			if timeoutErr.Dur != client.timeout {
				t.Errorf("TimeoutError.Dur = %v, want %v", timeoutErr.Dur, client.timeout)
			}
		})

		t.Run(op.name+"/context cancelled", func(t *testing.T) {
			client := causeClient(t, op.answers)
			client.timeout = time.Minute
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
			defer cancel()
			err := op.run(ctx, client)

			if _, ok := op.wrapper(err); !ok {
				t.Fatalf("error = %v (%T), want the operation's own error type", err, err)
			}
			if !errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("errors.Is(%v, context.DeadlineExceeded) = false", err)
			}
		})

		t.Run(op.name+"/closed connection", func(t *testing.T) {
			client := causeClient(t, op.answers)
			// A pattern subscribe needs its first request to get through.
			if op.answers == nil {
				_ = client.conn.Close()
			} else {
				client.timeout = time.Minute
				go func() {
					time.Sleep(30 * time.Millisecond)
					client.close()
				}()
			}
			err := op.run(context.Background(), client)

			if _, ok := op.wrapper(err); !ok {
				t.Fatalf("error = %v (%T), want the operation's own error type", err, err)
			}
			var connErr *ConnectionError
			if !errors.As(err, &connErr) {
				t.Fatalf("errors.As(%v, *ConnectionError) = false", err)
			}
		})

		t.Run(op.name+"/disposed", func(t *testing.T) {
			client := causeClient(t, op.answers)
			client.close()
			err := op.run(context.Background(), client)

			var disposedErr *DisposedError
			if !errors.As(err, &disposedErr) {
				t.Fatalf("errors.As(%v, *DisposedError) = false", err)
			}
		})
	}
}

func TestFireAndForgetPublishKeepsItsCause(t *testing.T) {
	t.Run("write failure", func(t *testing.T) {
		client := causeClient(t, nil)
		_ = client.conn.Close()
		err := client.publishInternal(causeMessage(t))

		var publishErr *PublishError
		if !errors.As(err, &publishErr) {
			t.Fatalf("error = %v (%T), want a PublishError", err, err)
		}
		if !errors.Is(err, io.ErrClosedPipe) {
			t.Fatalf("errors.Is(%v, io.ErrClosedPipe) = false", err)
		}
	})

	t.Run("payload that cannot be encoded", func(t *testing.T) {
		client := causeClient(t, nil)
		err := client.publishInternal(causeMessage(t).WithPayload(make(chan int)))

		var publishErr *PublishError
		if !errors.As(err, &publishErr) {
			t.Fatalf("error = %v (%T), want a PublishError", err, err)
		}
		if publishErr.Err == nil {
			t.Fatal("PublishError.Err = nil, want the encoder's error")
		}
	})
}

// An engine rejection has no Go error underneath it.
func TestOperationErrorsWithoutACause(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want string
	}{
		{"subscription", &SubscriptionError{Msg: "denied"}, "subscription failed: denied"},
		{"publish", &PublishError{Msg: "denied"}, "publish failed: denied"},
		{"discovery", &DiscoveryError{Msg: "denied"}, "discovery failed: denied"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.err.Error(); got != tt.want {
				t.Errorf("Error() = %q, want %q", got, tt.want)
			}
			if cause := errors.Unwrap(tt.err); cause != nil {
				t.Errorf("Unwrap() = %v, want nil", cause)
			}
		})
	}
}

// refusedSocket is the path of a socket file nothing listens on any more.
func refusedSocket(t *testing.T) string {
	t.Helper()
	path := filepath.Join(socketDir(t), "r.sock")
	listener, err := net.ListenUnix("unix", &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		t.Fatalf("listen failed: %v", err)
	}
	// Keep the file when the listener closes, so the dial finds a socket and
	// is refused instead of finding nothing.
	listener.SetUnlinkOnClose(false)
	if err := listener.Close(); err != nil {
		t.Fatalf("close failed: %v", err)
	}
	return path
}

// A ConnectionError keeps the error underneath it at every site that has one.
func TestConnectionErrorKeepsItsCause(t *testing.T) {
	t.Run("dial refused", func(t *testing.T) {
		client, _ := unitClient(t)
		path := refusedSocket(t)
		err := client.connect(&ConnectOptions{SocketPath: path})

		var connErr *ConnectionError
		if !errors.As(err, &connErr) {
			t.Fatalf("error = %v (%T), want a ConnectionError", err, err)
		}
		if !errors.Is(err, syscall.ECONNREFUSED) {
			t.Errorf("errors.Is(%v, ECONNREFUSED) = false", err)
		}
		var opErr *net.OpError
		if !errors.As(err, &opErr) {
			t.Errorf("errors.As(%v, *net.OpError) = false", err)
		}
		want := "connection failed: failed to connect to " + path + ": "
		if !strings.HasPrefix(err.Error(), want) {
			t.Errorf("Error() = %q, want the prefix %q", err.Error(), want)
		}
	})

	queries := []struct {
		name string
		run  func(ctx context.Context, client *baseClient) error
	}{
		{"topology query", func(ctx context.Context, client *baseClient) error {
			_, err := client.getTopologyInternal(ctx)
			return err
		}},
		{"subscriptions query", func(ctx context.Context, client *baseClient) error {
			_, err := client.getMySubscriptionsInternal(ctx)
			return err
		}},
	}
	for _, query := range queries {
		t.Run(query.name+"/response subscription times out", func(t *testing.T) {
			client := causeClient(t, nil)
			err := query.run(context.Background(), client)

			var connErr *ConnectionError
			if !errors.As(err, &connErr) {
				t.Fatalf("error = %v (%T), want a ConnectionError", err, err)
			}
			var timeoutErr *TimeoutError
			if !errors.As(err, &timeoutErr) {
				t.Errorf("errors.As(%v, *TimeoutError) = false", err)
			}
			want := "connection failed: failed to subscribe to response type: timeout after 50ms: request timed out"
			if err.Error() != want {
				t.Errorf("Error() = %q, want %q", err.Error(), want)
			}
		})

		t.Run(query.name+"/send fails", func(t *testing.T) {
			client := causeClient(t, nil)
			_ = client.conn.Close()
			err := query.run(context.Background(), client)

			if !errors.Is(err, io.ErrClosedPipe) {
				t.Errorf("errors.Is(%v, io.ErrClosedPipe) = false", err)
			}
		})
	}

	// The failed send sits two levels down: the operation's error wraps a
	// ConnectionError, which wraps the write error.
	t.Run("send fails under a subscribe", func(t *testing.T) {
		client := causeClient(t, nil)
		_ = client.conn.Close()
		_, err := client.subscribeInternal(context.Background(), []string{"go78.event"})

		var connErr *ConnectionError
		if !errors.As(err, &connErr) {
			t.Fatalf("error = %v (%T), want a ConnectionError underneath", err, err)
		}
		if !errors.Is(err, io.ErrClosedPipe) {
			t.Errorf("errors.Is(%v, io.ErrClosedPipe) = false", err)
		}
		want := "connection failed: failed to send: io: read/write on closed pipe"
		if connErr.Error() != want {
			t.Errorf("ConnectionError.Error() = %q, want %q", connErr.Error(), want)
		}
	})
}

// Sites with no error underneath leave the cause nil.
func TestConnectionErrorWithoutACause(t *testing.T) {
	t.Run("not connected", func(t *testing.T) {
		client, _ := unitClient(t)
		_, err := client.discoverInternal(context.Background())

		var connErr *ConnectionError
		if !errors.As(err, &connErr) {
			t.Fatalf("error = %v (%T), want a ConnectionError", err, err)
		}
		if cause := errors.Unwrap(connErr); cause != nil {
			t.Errorf("Unwrap() = %v, want nil", cause)
		}
	})

	t.Run("connection closed while waiting", func(t *testing.T) {
		client := causeClient(t, nil)
		client.timeout = time.Minute
		go func() {
			time.Sleep(30 * time.Millisecond)
			client.close()
		}()
		_, err := client.discoverInternal(context.Background())

		var connErr *ConnectionError
		if !errors.As(err, &connErr) {
			t.Fatalf("error = %v (%T), want a ConnectionError", err, err)
		}
		if cause := errors.Unwrap(connErr); cause != nil {
			t.Errorf("Unwrap() = %v, want nil", cause)
		}
		if connErr.Error() != "connection failed: connection closed" {
			t.Errorf("Error() = %q", connErr.Error())
		}
	})

	t.Run("a literal", func(t *testing.T) {
		err := &ConnectionError{Msg: "denied"}
		if err.Error() != "connection failed: denied" {
			t.Errorf("Error() = %q", err.Error())
		}
		if cause := errors.Unwrap(err); cause != nil {
			t.Errorf("Unwrap() = %v, want nil", cause)
		}
	})
}
