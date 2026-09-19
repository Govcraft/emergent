package emergent

import (
	"bytes"
	"context"
	"errors"
	"log"
	"reflect"
	"strings"
	"testing"
	"time"
)

// engineErrorBody is the body of an ERROR frame captured from a real engine.
func engineErrorBody(correlationID string) map[string]any {
	return map[string]any{
		"correlation_id": correlationID,
		"success":        false,
		"error":          fakeRejectionError,
		"error_code":     fakeRejectionCode,
	}
}

func TestResponseFromFrame(t *testing.T) {
	tests := []struct {
		name    string
		msgType byte
		payload any
		want    *IpcResponse
	}{
		{
			"error frame reads as a failure with its text",
			MsgTypeError, engineErrorBody("req_x"),
			&IpcResponse{CorrelationID: "req_x", Error: fakeRejectionError, ErrorCode: fakeRejectionCode},
		},
		{
			"error frame is a failure whatever its success field says",
			MsgTypeError, map[string]any{"correlation_id": "req_x", "success": true, "error": "boom"},
			&IpcResponse{CorrelationID: "req_x", Error: "boom"},
		},
		{
			"error frame without text gets the default text",
			MsgTypeError, map[string]any{"correlation_id": "req_x"},
			&IpcResponse{CorrelationID: "req_x", Error: defaultErrorText},
		},
		{
			"response frame is read as sent",
			MsgTypeResponse, map[string]any{"correlation_id": "req_x", "success": true, "payload": map[string]any{"n": 1}},
			&IpcResponse{CorrelationID: "req_x", Success: true, Payload: map[string]any{"n": 1}},
		},
		{
			"failed response frame keeps an empty error empty",
			MsgTypeResponse, map[string]any{"correlation_id": "req_x", "success": false},
			&IpcResponse{CorrelationID: "req_x"},
		},
		{"body without a correlation id", MsgTypeError, map[string]any{"success": false, "error": "boom"}, nil},
		{"correlation id that is not a string", MsgTypeResponse, map[string]any{"correlation_id": 7}, nil},
		{"empty correlation id", MsgTypeResponse, map[string]any{"correlation_id": ""}, nil},
		{"body that is not a map", MsgTypeError, "boom", nil},
		{"nil body", MsgTypeError, nil, nil},
		{"push frame", MsgTypePush, engineErrorBody("req_x"), nil},
		{"heartbeat frame", MsgTypeHeartbeat, nil, nil},
		{"stream frame", MsgTypeStream, map[string]any{"correlation_id": "str_1"}, nil},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok := responseFromFrame(tt.msgType, tt.payload)
			if got != nil {
				// Body is the input itself. TestResponseFromFrame_KeepsTheBody
				// covers it, so it is left out of the comparison here.
				got.Body = nil
			}
			if ok != (tt.want != nil) || !reflect.DeepEqual(got, tt.want) {
				t.Errorf("responseFromFrame() = (%+v, %v), want %+v", got, ok, tt.want)
			}
		})
	}
}

// TestResponseFromFrame_KeepsTheBody checks that fields beside the shared
// ones survive, since discovery answers at the top level of the body.
func TestResponseFromFrame_KeepsTheBody(t *testing.T) {
	body := map[string]any{"correlation_id": "disc_1", "success": true, "message_types": []any{"SystemEvent"}}
	resp, ok := responseFromFrame(MsgTypeResponse, body)
	if !ok {
		t.Fatal("expected a response")
	}
	if !reflect.DeepEqual(resp.Body, body) {
		t.Errorf("Body = %v, want %v", resp.Body, body)
	}
}

func TestErrorFrameText(t *testing.T) {
	tests := []struct {
		name    string
		payload any
		want    string
	}{
		{"text and code", engineErrorBody("req_x"), "Actor not found: no_such_actor (ACTOR_NOT_FOUND)"},
		{"text alone", map[string]any{"error": "boom"}, "boom"},
		{"code alone", map[string]any{"error_code": "E1"}, defaultErrorText + " (E1)"},
		{"empty text", map[string]any{"error": ""}, defaultErrorText},
		{"text that is not a string", map[string]any{"error": 7}, defaultErrorText},
		{"no body", nil, defaultErrorText},
		{"body that is not a map", "boom", defaultErrorText},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := errorFrameText(tt.payload); got != tt.want {
				t.Errorf("errorFrameText() = %q, want %q", got, tt.want)
			}
		})
	}
}

// unitClient is a baseClient with no socket whose log lines land in a buffer.
func unitClient(t *testing.T) (*baseClient, *bytes.Buffer) {
	t.Helper()
	t.Setenv("EMERGENT_LOG", "off")
	client := newBaseClient("unit_handler", PrimitiveKindHandler, nil)
	logs := &bytes.Buffer{}
	client.logger = &Logger{name: "unit_handler", level: LogLevelWarn, logger: log.New(logs, "", 0)}
	return client, logs
}

// addPending registers a pending request the way sendRequest does.
func addPending(client *baseClient, correlationID string) (chan ipcResponseResult, *time.Timer) {
	ch := make(chan ipcResponseResult, 1)
	timer := time.NewTimer(time.Hour)
	client.mu.Lock()
	client.pendingRequests[correlationID] = &pendingRequest{ch: ch, timer: timer}
	client.mu.Unlock()
	return ch, timer
}

// feedFrames hands raw bytes to the client as if the read loop had read them.
func feedFrames(client *baseClient, raw []byte) {
	client.mu.Lock()
	client.readBuffer = append(client.readBuffer, raw...)
	client.mu.Unlock()
	client.processFrames()
}

func mustEncode(t *testing.T, msgType byte, payload any) []byte {
	t.Helper()
	frame, err := EncodeFrame(msgType, payload, FormatMsgPack)
	if err != nil {
		t.Fatalf("encode failed: %v", err)
	}
	return frame
}

func TestErrorFrameSettlesItsRequest(t *testing.T) {
	client, _ := unitClient(t)
	mine, mineTimer := addPending(client, "req_mine")
	other, otherTimer := addPending(client, "req_other")
	defer otherTimer.Stop()

	feedFrames(client, mustEncode(t, MsgTypeError, engineErrorBody("req_mine")))

	select {
	case result := <-mine:
		if result.err != nil || result.response == nil {
			t.Fatalf("unexpected result: %+v", result)
		}
		if result.response.Success || result.response.Error != fakeRejectionError || result.response.ErrorCode != fakeRejectionCode {
			t.Fatalf("unexpected response: %+v", result.response)
		}
	default:
		t.Fatal("the ERROR frame did not settle its request")
	}
	if mineTimer.Stop() {
		t.Error("the request timer was left running")
	}

	select {
	case result := <-other:
		t.Fatalf("an ERROR frame for another request settled this one: %+v", result)
	default:
	}
	client.mu.Lock()
	_, mineLeft := client.pendingRequests["req_mine"]
	_, otherLeft := client.pendingRequests["req_other"]
	client.mu.Unlock()
	if mineLeft || !otherLeft {
		t.Errorf("pending entries: req_mine=%v req_other=%v, want false and true", mineLeft, otherLeft)
	}
}

func TestUnmatchedErrorFrameIsLogged(t *testing.T) {
	tests := []struct {
		name    string
		payload any
		wantID  string
	}{
		{
			"connection-limit rejection sentinel",
			map[string]any{"correlation_id": "__acton_connection_rejected__", "success": false, "error": "Connection limit reached", "error_code": "CONNECTION_LIMIT"},
			"correlation_id=__acton_connection_rejected__",
		},
		{"request that already timed out", engineErrorBody("req_gone"), "correlation_id=req_gone"},
		{"no correlation id", map[string]any{"success": false, "error": fakeRejectionError, "error_code": fakeRejectionCode}, "correlation_id=unknown"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client, logs := unitClient(t)
			waiting, timer := addPending(client, "req_waiting")
			defer timer.Stop()

			feedFrames(client, mustEncode(t, MsgTypeError, tt.payload))

			line := logs.String()
			if !strings.Contains(line, "ERROR") || !strings.Contains(line, "matched no pending request") || !strings.Contains(line, tt.wantID) {
				t.Errorf("unexpected log output: %q", line)
			}
			if !strings.Contains(line, errorFrameText(tt.payload)) {
				t.Errorf("log output lacks the engine's text: %q", line)
			}
			select {
			case result := <-waiting:
				t.Fatalf("an unmatched ERROR frame settled a request: %+v", result)
			default:
			}
		})
	}
}

// TestIgnoredFramesDoNotStopTheReadLoop feeds a heartbeat, a frame of unknown
// type and a stream frame ahead of a real answer. None of them may settle the
// request, and none may make the client throw away the rest of the buffer.
func TestIgnoredFramesDoNotStopTheReadLoop(t *testing.T) {
	client, logs := unitClient(t)
	waiting, timer := addPending(client, "req_1")
	defer timer.Stop()

	heartbeat := []byte{0x00, 0x00, 0x00, 0x00, ProtocolVersion, MsgTypeHeartbeat, FormatJSON}
	unknown := []byte{0x00, 0x00, 0x00, 0x02, ProtocolVersion, 0x0c, FormatJSON, '{', '}'}
	stream := mustEncode(t, MsgTypeStream, map[string]any{"correlation_id": "str_1", "sequence": 0, "is_final": true})
	ok := mustEncode(t, MsgTypeResponse, map[string]any{"correlation_id": "req_1", "success": true})

	var raw []byte
	for _, frame := range [][]byte{heartbeat, unknown, stream, ok} {
		raw = append(raw, frame...)
	}
	feedFrames(client, raw)

	select {
	case result := <-waiting:
		if result.response == nil || !result.response.Success {
			t.Fatalf("unexpected result: %+v", result)
		}
	default:
		t.Fatal("the answer behind the ignored frames was lost")
	}

	client.mu.Lock()
	left := len(client.readBuffer)
	client.mu.Unlock()
	if left != 0 {
		t.Errorf("%d bytes left in the read buffer", left)
	}
	if line := logs.String(); !strings.Contains(line, "unknown type") || !strings.Contains(line, "0x0c") {
		t.Errorf("the unknown frame type was not logged: %q", line)
	}
	if strings.Contains(logs.String(), "0x04") || strings.Contains(logs.String(), "0x09") {
		t.Errorf("heartbeat or stream frames were logged above debug level: %q", logs.String())
	}
}

func TestMalformedResponseBodyIsDropped(t *testing.T) {
	client, logs := unitClient(t)
	feedFrames(client, mustEncode(t, MsgTypeResponse, map[string]any{"success": true}))
	if !strings.Contains(logs.String(), "malformed response") {
		t.Errorf("unexpected log output: %q", logs.String())
	}
}

// TestRejectedRequestsFailWithTheEngineText runs real calls against a fake
// engine that answers with ERROR frames. The client timeout is a minute, so a
// call that returns at all was settled by the ERROR frame.
func TestRejectedRequestsFailWithTheEngineText(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	engine.rejectRequests.Store(true)
	handler, err := ConnectHandler("rejected_handler", &ConnectOptions{SocketPath: engine.socketPath, Timeout: time.Minute})
	if err != nil {
		t.Fatalf("failed to connect handler: %v", err)
	}
	t.Cleanup(func() { _ = handler.Close() })

	calls := []struct {
		name string
		call func() error
		is   func(error) bool
	}{
		{"PublishAck", func() error {
			msg, msgErr := NewMessage("rejected.event")
			if msgErr != nil {
				return msgErr
			}
			return handler.PublishAck(context.Background(), msg)
		}, func(err error) bool { var target *PublishError; return errors.As(err, &target) }},
		{"Subscribe", func() error {
			_, subErr := handler.Subscribe(context.Background(), []string{"rejected.event"})
			return subErr
		}, func(err error) bool { var target *SubscriptionError; return errors.As(err, &target) }},
		{"Discover", func() error {
			_, discoverErr := handler.Discover(context.Background())
			return discoverErr
		}, func(err error) bool { var target *DiscoveryError; return errors.As(err, &target) }},
	}

	for _, tt := range calls {
		t.Run(tt.name, func(t *testing.T) {
			done := make(chan error, 1)
			go func() { done <- tt.call() }()

			select {
			case callErr := <-done:
				if callErr == nil || !tt.is(callErr) {
					t.Fatalf("unexpected error type: %v", callErr)
				}
				if !strings.Contains(callErr.Error(), fakeRejectionError) {
					t.Fatalf("the engine's text was lost: %v", callErr)
				}
			case <-time.After(streamDeadline):
				t.Fatal("the call ignored the ERROR frame and is waiting for its timeout")
			}
			assertNoPendingRequests(t, handler.baseClient)
		})
	}
}
