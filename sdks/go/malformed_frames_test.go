package emergent

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"maps"
	"testing"
)

// One bad frame must never stop the frames behind it. It is logged and
// skipped, and a good frame in the same chunk is still delivered.

func goodWireMessage() map[string]any {
	return map[string]any{
		"id":           "msg_01",
		"message_type": "go64.event",
		"source":       "upstream",
		"timestamp_ms": 1700000000000,
		"payload":      map[string]any{"n": 1},
	}
}

func goodNotification() map[string]any {
	return map[string]any{
		"notification_id": "ntf_01",
		"message_type":    "go64.event",
		"timestamp_ms":    1700000000001,
		"payload":         goodWireMessage(),
	}
}

// withField returns a copy of base with one field replaced.
func withField(base map[string]any, field string, value any) map[string]any {
	out := maps.Clone(base)
	out[field] = value
	return out
}

// withoutField returns a copy of base with one field removed.
func withoutField(base map[string]any, field string) map[string]any {
	out := maps.Clone(base)
	delete(out, field)
	return out
}

// rawFrame builds a frame around the given body bytes, whatever they hold.
func rawFrame(msgType, format byte, body []byte) []byte {
	frame := make([]byte, HeaderSize+len(body))
	binary.BigEndian.PutUint32(frame[0:4], uint32(len(body)))
	frame[4], frame[5], frame[6] = ProtocolVersion, msgType, format
	copy(frame[HeaderSize:], body)
	return frame
}

func truncatedMsgPack(t *testing.T) []byte {
	t.Helper()
	frame := mustEncode(t, MsgTypePush, goodNotification())
	return frame[HeaderSize : len(frame)-5]
}

func truncatedJSON(t *testing.T) []byte {
	t.Helper()
	body, err := json.Marshal(goodNotification())
	if err != nil {
		t.Fatalf("marshal failed: %v", err)
	}
	return body[:len(body)-5]
}

func TestNextFrameReportsABadBodyWithTheLengthToSkip(t *testing.T) {
	tests := []struct {
		name   string
		format byte
		body   []byte
	}{
		{"truncated MessagePack", FormatMsgPack, truncatedMsgPack(t)},
		{"MessagePack cut after one byte", FormatMsgPack, truncatedMsgPack(t)[:1]},
		{"truncated JSON", FormatJSON, truncatedJSON(t)},
		{"JSON that is not JSON", FormatJSON, []byte("nope")},
		{"unknown format", 0x09, []byte("{}")},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			frame := rawFrame(MsgTypePush, tt.format, tt.body)
			step := nextFrame(append(frame, mustEncode(t, MsgTypePush, goodNotification())...))
			if step.kind != frameBadBody {
				t.Fatalf("kind = %v, want frameBadBody", step.kind)
			}
			if step.msgType != MsgTypePush || step.bytesConsumed != len(frame) || step.reason == "" {
				t.Errorf("unexpected step: %+v", step)
			}

			var protocolErr *ProtocolError
			if _, err := TryDecodeFrame(frame); !errors.As(err, &protocolErr) {
				t.Errorf("TryDecodeFrame error = %v, want a *ProtocolError", err)
			}
		})
	}
}

func TestNextFrameReportsAHeaderItCannotTrust(t *testing.T) {
	tooLarge := make([]byte, HeaderSize)
	binary.BigEndian.PutUint32(tooLarge[0:4], uint32(MaxFrameSize+1))
	tooLarge[4] = ProtocolVersion

	tests := []struct {
		name   string
		buffer []byte
	}{
		{"too large", tooLarge},
		{"wrong version", []byte{0, 0, 0, 2, 0x01, MsgTypePush, FormatJSON, '{', '}'}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if step := nextFrame(tt.buffer); step.kind != frameBadFraming || step.reason == "" {
				t.Errorf("unexpected step: %+v", step)
			}
		})
	}
}

func TestNextFrameWaitsForAWholeFrame(t *testing.T) {
	frame := mustEncode(t, MsgTypePush, goodNotification())
	for _, length := range []int{0, 3, HeaderSize, len(frame) - 1} {
		if step := nextFrame(frame[:length]); step.kind != frameIncomplete {
			t.Errorf("length %d: kind = %v, want frameIncomplete", length, step.kind)
		}
	}
	if step := nextFrame(frame); step.kind != frameDecoded || step.frame.BytesConsumed != len(frame) {
		t.Errorf("whole frame: unexpected step: %+v", step)
	}
}

func TestWireMessageFromPush(t *testing.T) {
	good := goodWireMessage()
	tests := []struct {
		name    string
		payload any
		want    bool
	}{
		{"whole message", good, true},
		{"with ids", withField(withField(good, "correlation_id", "cor_1"), "causation_id", "msg_00"), true},
		{"nil optional ids", withField(withField(good, "correlation_id", nil), "causation_id", nil), true},
		{"JSON timestamp", withField(good, "timestamp_ms", float64(1700000000000)), true},
		{"nil payload", withField(good, "payload", nil), true},
		{"nil", nil, false},
		{"text", "text", false},
		{"list", []any{good}, false},
		{"empty", map[string]any{}, false},
		{"no id", withoutField(good, "id"), false},
		{"no timestamp", withoutField(good, "timestamp_ms"), false},
		{"numeric id", withField(good, "id", 1), false},
		{"numeric message_type", withField(good, "message_type", 2), false},
		{"list source", withField(good, "source", []any{}), false},
		{"text timestamp", withField(good, "timestamp_ms", "x"), false},
		{"negative timestamp", withField(good, "timestamp_ms", -1), false},
		{"numeric correlation_id", withField(good, "correlation_id", 9), false},
		{"map causation_id", withField(good, "causation_id", map[string]any{}), false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			wire, ok := wireMessageFromPush(tt.payload)
			if ok != tt.want {
				t.Fatalf("ok = %v, want %v", ok, tt.want)
			}
			if ok && wire["id"] != "msg_01" {
				t.Errorf("unexpected message: %+v", wire)
			}
		})
	}
}

func TestAGoodFrameBehindAMalformedOneIsStillDelivered(t *testing.T) {
	note := goodNotification()
	tests := []struct {
		name string
		bad  []byte
	}{
		{"PUSH with a nil body", mustEncode(t, MsgTypePush, nil)},
		{"PUSH with a list body", mustEncode(t, MsgTypePush, []any{note})},
		{"PUSH with a numeric message_type", mustEncode(t, MsgTypePush, withField(note, "message_type", 7))},
		{"PUSH with a nil message", mustEncode(t, MsgTypePush, withField(note, "payload", nil))},
		{"PUSH with wrong-typed message fields", mustEncode(t, MsgTypePush, withField(note, "payload",
			map[string]any{"id": 1, "message_type": 2, "source": []any{}, "timestamp_ms": "x"}))},
		{"topology response with a nil message", mustEncode(t, MsgTypePush,
			withField(withField(note, "message_type", "system.response.topology"), "payload", nil))},
		{"subscriptions response with a nil message", mustEncode(t, MsgTypePush,
			withField(withField(note, "message_type", "system.response.subscriptions"), "payload", nil))},
		{"PUSH with a truncated MessagePack body", rawFrame(MsgTypePush, FormatMsgPack, truncatedMsgPack(t))},
		{"RESPONSE with a truncated MessagePack body", rawFrame(MsgTypeResponse, FormatMsgPack, truncatedMsgPack(t))},
		{"PUSH with a truncated JSON body", rawFrame(MsgTypePush, FormatJSON, truncatedJSON(t))},
		{"PUSH in an unknown format", rawFrame(MsgTypePush, 0x09, []byte("{}"))},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client, _ := unitClient(t)
			stream := newMessageStream(16, nil)
			client.messageStream = stream

			// One chunk, so a frame that emptied the buffer would lose the good one.
			feedFrames(client, append(append([]byte{}, tt.bad...), mustEncode(t, MsgTypePush, note)...))

			if stream.IsClosed() {
				t.Fatal("the stream was closed")
			}
			if got := stream.Pending(); got != 1 {
				t.Fatalf("pending = %d, want 1", got)
			}
			if msg := stream.TryNext(); msg == nil || msg.ID != "msg_01" {
				t.Errorf("unexpected message: %+v", msg)
			}
		})
	}
}

func TestAMalformedFrameSplitAcrossChunksIsSkipped(t *testing.T) {
	client, _ := unitClient(t)
	stream := newMessageStream(16, nil)
	client.messageStream = stream
	chunk := append(rawFrame(MsgTypePush, FormatMsgPack, truncatedMsgPack(t)),
		mustEncode(t, MsgTypePush, goodNotification())...)

	feedFrames(client, chunk[:20])
	if got := stream.Pending(); got != 0 {
		t.Fatalf("pending after half a frame = %d, want 0", got)
	}
	feedFrames(client, chunk[20:])

	if msg := stream.TryNext(); msg == nil || msg.ID != "msg_01" {
		t.Errorf("unexpected message: %+v", msg)
	}
}
