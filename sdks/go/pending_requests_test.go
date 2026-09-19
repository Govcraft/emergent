package emergent

import (
	"context"
	"errors"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// fakeReply is one frame the fake engine sends back for an incoming frame.
type fakeReply struct {
	msgType byte
	payload any
	// delayed marks a pub/sub answer, which the fake engine may hold back to
	// collide with the client's request timeout.
	delayed bool
}

// fakeTopologyPrimitives is the topology the fake engine reports.
func fakeTopologyPrimitives() []any {
	return []any{
		map[string]any{
			"name":       "timer",
			"kind":       "source",
			"state":      "running",
			"publishes":  []any{"timer.tick"},
			"subscribes": []any{},
		},
		map[string]any{
			"name":       "console",
			"kind":       "sink",
			"state":      "running",
			"publishes":  []any{},
			"subscribes": []any{"timer.tick"},
		},
	}
}

// fakeEngineReply decides how the fake engine answers one decoded frame.
// It is pure: the same frame always yields the same reply.
func fakeEngineReply(msgType byte, payload any) (fakeReply, bool) {
	payloadMap, ok := payload.(map[string]any)
	if !ok {
		return fakeReply{}, false
	}

	switch msgType {
	case MsgTypeSubscribe, MsgTypeUnsubscribe:
		correlationID, _ := payloadMap["correlation_id"].(string)
		return fakeReply{
			msgType: MsgTypeResponse,
			payload: &IpcResponse{CorrelationID: correlationID, Success: true},
		}, true
	case MsgTypeRequest:
		envelopePayload, _ := payloadMap["payload"].(map[string]any)
		inner, _ := envelopePayload["inner"].(map[string]any)
		messageType, _ := inner["message_type"].(string)
		correlationID, _ := inner["correlation_id"].(string)

		var responseType string
		var responsePayload map[string]any
		switch messageType {
		case "system.request.topology":
			responseType = "system.response.topology"
			responsePayload = map[string]any{"primitives": fakeTopologyPrimitives()}
		case "system.request.subscriptions":
			responseType = "system.response.subscriptions"
			responsePayload = map[string]any{"subscribes": []any{"timer.tick", "timer.filtered"}}
		default:
			return fakeReply{}, false
		}

		return fakeReply{
			msgType: MsgTypePush,
			payload: &IpcPushNotification{
				NotificationID: "ntf_fake",
				MessageType:    responseType,
				Payload: map[string]any{
					"id":             "msg_fake",
					"message_type":   responseType,
					"source":         "emergent-engine",
					"correlation_id": correlationID,
					"timestamp_ms":   uint64(1),
					"payload":        responsePayload,
				},
			},
			delayed: true,
		}, true
	default:
		return fakeReply{}, false
	}
}

func TestFakeEngineReply(t *testing.T) {
	t.Run("subscribe is acknowledged with the same correlation id", func(t *testing.T) {
		reply, ok := fakeEngineReply(MsgTypeSubscribe, map[string]any{"correlation_id": "sub_1"})
		if !ok {
			t.Fatal("expected a reply")
		}
		resp, isResp := reply.payload.(*IpcResponse)
		if !isResp || reply.msgType != MsgTypeResponse || resp.CorrelationID != "sub_1" || !resp.Success {
			t.Fatalf("unexpected reply: %+v", reply)
		}
	})

	t.Run("topology request is answered with a correlated push", func(t *testing.T) {
		reply, ok := fakeEngineReply(MsgTypeRequest, map[string]any{
			"payload": map[string]any{"inner": map[string]any{
				"message_type":   "system.request.topology",
				"correlation_id": "cor_1",
			}},
		})
		if !ok {
			t.Fatal("expected a reply")
		}
		push, isPush := reply.payload.(*IpcPushNotification)
		if !isPush || reply.msgType != MsgTypePush || push.MessageType != "system.response.topology" {
			t.Fatalf("unexpected reply: %+v", reply)
		}
		wire, _ := push.Payload.(map[string]any)
		if wire["correlation_id"] != "cor_1" {
			t.Fatalf("correlation id not echoed: %+v", wire)
		}
	})

	t.Run("other published messages get no reply", func(t *testing.T) {
		_, ok := fakeEngineReply(MsgTypeRequest, map[string]any{
			"payload": map[string]any{"inner": map[string]any{"message_type": "timer.tick"}},
		})
		if ok {
			t.Fatal("expected no reply")
		}
	})
}

// fakeEngine is an in-process stand-in for the engine's IPC socket. It speaks
// just enough of the wire protocol to answer subscribe requests and the two
// pub/sub lookups (topology and subscriptions).
type fakeEngine struct {
	socketPath string
	listener   net.Listener
	// pushDelay holds back pub/sub answers so they land around the client's
	// request timeout.
	pushDelay time.Duration

	mu    sync.Mutex
	conns []*fakeConn
	wg    sync.WaitGroup
}

// fakeConn is one accepted client connection. Its mutex keeps frames whole
// when a reply and a broadcast are written at the same time.
type fakeConn struct {
	net.Conn
	writeMu sync.Mutex
}

func (fc *fakeConn) send(reply fakeReply) {
	frame, err := EncodeFrame(reply.msgType, reply.payload, FormatMsgPack)
	if err != nil {
		return
	}
	fc.writeMu.Lock()
	_, _ = fc.Write(frame)
	fc.writeMu.Unlock()
}

func startFakeEngine(t *testing.T, pushDelay time.Duration) *fakeEngine {
	t.Helper()

	// Unix socket paths are limited to about 108 bytes, so keep this short
	// instead of using t.TempDir(), which embeds the full test name.
	dir, err := os.MkdirTemp("", "emg-*")
	if err != nil {
		t.Fatalf("failed to create temp dir: %v", err)
	}
	socketPath := filepath.Join(dir, "e.sock")

	listener, err := net.Listen("unix", socketPath)
	if err != nil {
		_ = os.RemoveAll(dir)
		t.Fatalf("failed to listen on %s: %v", socketPath, err)
	}

	engine := &fakeEngine{socketPath: socketPath, listener: listener, pushDelay: pushDelay}
	engine.wg.Add(1)
	go engine.acceptLoop()

	t.Cleanup(func() {
		engine.stop()
		_ = os.RemoveAll(dir)
	})
	return engine
}

func (e *fakeEngine) stop() {
	_ = e.listener.Close()
	e.mu.Lock()
	for _, conn := range e.conns {
		_ = conn.Close()
	}
	e.mu.Unlock()
	e.wg.Wait()
}

// connections returns the client connections accepted so far.
func (e *fakeEngine) connections() []*fakeConn {
	e.mu.Lock()
	defer e.mu.Unlock()
	return append([]*fakeConn(nil), e.conns...)
}

// broadcast pushes one frame to every connected client, the way the engine
// fans a system event out to its subscribers.
func (e *fakeEngine) broadcast(reply fakeReply) {
	for _, conn := range e.connections() {
		conn.send(reply)
	}
}

// dropConnections closes every client connection from the engine side, which
// is what a client sees when the engine dies.
func (e *fakeEngine) dropConnections() {
	for _, conn := range e.connections() {
		_ = conn.Close()
	}
}

func (e *fakeEngine) acceptLoop() {
	defer e.wg.Done()
	for {
		accepted, err := e.listener.Accept()
		if err != nil {
			return
		}
		conn := &fakeConn{Conn: accepted}
		e.mu.Lock()
		e.conns = append(e.conns, conn)
		e.mu.Unlock()

		e.wg.Add(1)
		go e.serve(conn)
	}
}

func (e *fakeEngine) serve(conn *fakeConn) {
	defer e.wg.Done()

	var buffer []byte
	chunk := make([]byte, 64*1024)
	for {
		n, err := conn.Read(chunk)
		if err != nil {
			return
		}
		buffer = append(buffer, chunk[:n]...)

		for {
			frame, decodeErr := TryDecodeFrame(buffer)
			if decodeErr != nil {
				return
			}
			if frame == nil {
				break
			}
			buffer = buffer[frame.BytesConsumed:]

			reply, ok := fakeEngineReply(frame.MsgType, frame.Payload)
			if !ok {
				continue
			}
			if reply.delayed && e.pushDelay > 0 {
				e.wg.Add(1)
				go func() {
					defer e.wg.Done()
					time.Sleep(e.pushDelay)
					conn.send(reply)
				}()
				continue
			}
			conn.send(reply)
		}
	}
}

func connectTestSink(t *testing.T, engine *fakeEngine, timeout time.Duration) *EmergentSink {
	t.Helper()
	sink, err := ConnectSink("race_sink", &ConnectOptions{SocketPath: engine.socketPath, Timeout: timeout})
	if err != nil {
		t.Fatalf("failed to connect sink: %v", err)
	}
	t.Cleanup(func() { _ = sink.Close() })
	return sink
}

// TestConcurrentPubSubLookups fires GetTopology and GetMySubscriptions from
// many goroutines at once. The response handlers run on the read loop while
// callers register requests, so `go test -race` fails here if any access to
// the pending-request maps escapes c.mu.
func TestConcurrentPubSubLookups(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink := connectTestSink(t, engine, 10*time.Second)

	const workers = 16
	const rounds = 25

	ctx := context.Background()
	errCh := make(chan error, 2*workers*rounds)
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(2)
		go func() {
			defer wg.Done()
			for j := 0; j < rounds; j++ {
				state, err := sink.GetTopology(ctx)
				if err != nil {
					errCh <- err
					return
				}
				if len(state.Primitives) != 2 || state.Primitives[0].Name != "timer" {
					errCh <- errors.New("unexpected topology")
					return
				}
			}
		}()
		go func() {
			defer wg.Done()
			for j := 0; j < rounds; j++ {
				subs, err := sink.GetMySubscriptions(ctx)
				if err != nil {
					errCh <- err
					return
				}
				if len(subs) != 2 || subs[0] != "timer.tick" {
					errCh <- errors.New("unexpected subscriptions")
					return
				}
			}
		}()
	}
	wg.Wait()
	close(errCh)

	for err := range errCh {
		t.Errorf("lookup failed: %v", err)
	}

	assertNoPendingRequests(t, sink.baseClient)
}

// TestPubSubLookupsRacingTimeout makes answers arrive at the moment the
// request timer fires, so the timer goroutine, the read loop and the callers
// all reach the pending-request maps together. Either outcome is valid for a
// single call: the answer wins, or the timeout wins.
func TestPubSubLookupsRacingTimeout(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	const timeout = 40 * time.Millisecond
	engine := startFakeEngine(t, timeout)
	sink := connectTestSink(t, engine, timeout)

	const workers = 8
	const rounds = 10

	ctx := context.Background()
	errCh := make(chan error, 2*workers*rounds)
	checkErr := func(err error) {
		if err != nil && !isTimeout(err) {
			errCh <- err
		}
	}

	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(2)
		go func() {
			defer wg.Done()
			for j := 0; j < rounds; j++ {
				_, err := sink.GetTopology(ctx)
				checkErr(err)
			}
		}()
		go func() {
			defer wg.Done()
			for j := 0; j < rounds; j++ {
				_, err := sink.GetMySubscriptions(ctx)
				checkErr(err)
			}
		}()
	}
	wg.Wait()
	close(errCh)

	for err := range errCh {
		t.Errorf("lookup failed with a non-timeout error: %v", err)
	}

	assertNoPendingRequests(t, sink.baseClient)
}

// isTimeout reports whether a lookup failed only because a timer won. The
// lookup's own timer yields a TimeoutError. The timer of its preliminary
// subscribe request is reported as a ConnectionError that quotes the timeout.
func isTimeout(err error) bool {
	var timeoutErr *TimeoutError
	if errors.As(err, &timeoutErr) {
		return true
	}
	var connErr *ConnectionError
	return errors.As(err, &connErr) && strings.Contains(connErr.Msg, "timed out")
}

// assertNoPendingRequests checks that every request was removed from its map,
// whether it was answered or timed out.
func assertNoPendingRequests(t *testing.T, c *baseClient) {
	t.Helper()
	c.mu.Lock()
	defer c.mu.Unlock()
	if n := len(c.pendingRequests); n != 0 {
		t.Errorf("pendingRequests leaked %d entries", n)
	}
	if n := len(c.pendingTopologyRequests); n != 0 {
		t.Errorf("pendingTopologyRequests leaked %d entries", n)
	}
	if n := len(c.pendingSubscriptionRequests); n != 0 {
		t.Errorf("pendingSubscriptionRequests leaked %d entries", n)
	}
}
