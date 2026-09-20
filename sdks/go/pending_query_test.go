package emergent

import (
	"context"
	"errors"
	"io"
	"net"
	"testing"
	"time"
)

// leavingEngine is the far end of a pipe that accepts the first request, the
// query's subscribe, and is gone before the request publish that follows.
func leavingEngine(t *testing.T, client *baseClient, server net.Conn) {
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
			frame, decodeErr := TryDecodeFrame(buffer)
			if decodeErr != nil || frame == nil {
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
			_ = server.Close()
			feedFrames(client, reply)
			return
		}
	}()
}

// A query registers its pending entry and timer, then publishes the request.
// A refused publish has to take both with it. The Python and TypeScript SDKs
// left them behind (Govcraft/emergent#79).
func TestARefusedQueryPublishLeavesNothingPending(t *testing.T) {
	queries := []struct {
		name string
		run  func(*baseClient) error
	}{
		{"topology", func(c *baseClient) error {
			_, err := c.getTopologyInternal(context.Background())
			return err
		}},
		{"subscriptions", func(c *baseClient) error {
			_, err := c.getMySubscriptionsInternal(context.Background())
			return err
		}},
	}
	for _, query := range queries {
		t.Run(query.name, func(t *testing.T) {
			client, _ := unitClient(t)
			client.timeout = 30 * time.Second
			conn, server := net.Pipe()
			t.Cleanup(func() {
				_ = conn.Close()
				_ = server.Close()
			})
			client.conn = conn
			// No read loop runs here, so close() has nothing to wait for.
			close(client.readDone)
			leavingEngine(t, client, server)

			err := query.run(client)

			var publishErr *PublishError
			if !errors.As(err, &publishErr) {
				t.Fatalf("got %T (%v), want a *PublishError", err, err)
			}
			if !errors.Is(err, io.ErrClosedPipe) {
				t.Errorf("errors.Is(err, io.ErrClosedPipe) = false for %v", err)
			}
			assertNoPendingRequests(t, client)
		})
	}
}
