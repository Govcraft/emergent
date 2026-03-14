package emergent

import (
	"testing"
)

func TestEncodeDecodeFrame_MsgPack(t *testing.T) {
	payload := map[string]any{
		"correlation_id": "req_abc123",
		"message_types":  []string{"timer.tick"},
	}

	frame, err := EncodeFrame(MsgTypeSubscribe, payload, FormatMsgPack)
	if err != nil {
		t.Fatalf("encode error: %v", err)
	}

	// Check header
	if frame[4] != ProtocolVersion {
		t.Errorf("expected version %d, got %d", ProtocolVersion, frame[4])
	}
	if frame[5] != MsgTypeSubscribe {
		t.Errorf("expected msg type %d, got %d", MsgTypeSubscribe, frame[5])
	}
	if frame[6] != FormatMsgPack {
		t.Errorf("expected format %d, got %d", FormatMsgPack, frame[6])
	}

	// Decode
	decoded, err := TryDecodeFrame(frame)
	if err != nil {
		t.Fatalf("decode error: %v", err)
	}
	if decoded == nil {
		t.Fatal("expected decoded frame")
	}
	if decoded.MsgType != MsgTypeSubscribe {
		t.Errorf("expected msg type %d, got %d", MsgTypeSubscribe, decoded.MsgType)
	}
	if decoded.Format != FormatMsgPack {
		t.Errorf("expected format %d, got %d", FormatMsgPack, decoded.Format)
	}
	if decoded.BytesConsumed != len(frame) {
		t.Errorf("expected %d bytes consumed, got %d", len(frame), decoded.BytesConsumed)
	}
}

func TestEncodeDecodeFrame_JSON(t *testing.T) {
	payload := map[string]any{
		"success":        true,
		"correlation_id": "sub_xyz",
	}

	frame, err := EncodeFrame(MsgTypeResponse, payload, FormatJSON)
	if err != nil {
		t.Fatalf("encode error: %v", err)
	}

	if frame[6] != FormatJSON {
		t.Errorf("expected JSON format, got %d", frame[6])
	}

	decoded, err := TryDecodeFrame(frame)
	if err != nil {
		t.Fatalf("decode error: %v", err)
	}

	payloadMap, ok := decoded.Payload.(map[string]any)
	if !ok {
		t.Fatal("expected map payload")
	}
	if payloadMap["success"] != true {
		t.Errorf("expected success=true, got %v", payloadMap["success"])
	}
	if payloadMap["correlation_id"] != "sub_xyz" {
		t.Errorf("expected sub_xyz, got %v", payloadMap["correlation_id"])
	}
}

func TestTryDecodeFrame_IncompleteHeader(t *testing.T) {
	// Less than 7 bytes
	result, err := TryDecodeFrame([]byte{0, 0, 0})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != nil {
		t.Error("expected nil for incomplete header")
	}
}

func TestTryDecodeFrame_IncompletePayload(t *testing.T) {
	// Header says 100 bytes payload, but only 3 bytes available
	frame := []byte{0, 0, 0, 100, ProtocolVersion, MsgTypeResponse, FormatJSON, 1, 2, 3}
	result, err := TryDecodeFrame(frame)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != nil {
		t.Error("expected nil for incomplete payload")
	}
}

func TestTryDecodeFrame_WrongVersion(t *testing.T) {
	frame := []byte{0, 0, 0, 2, 0x99, MsgTypeResponse, FormatJSON, '{', '}'}
	_, err := TryDecodeFrame(frame)
	if err == nil {
		t.Error("expected error for wrong version")
	}
}

func TestTryDecodeFrame_UnsupportedFormat(t *testing.T) {
	frame := []byte{0, 0, 0, 2, ProtocolVersion, MsgTypeResponse, 0xFF, '{', '}'}
	_, err := TryDecodeFrame(frame)
	if err == nil {
		t.Error("expected error for unsupported format")
	}
}

func TestEncodeFrame_UnsupportedFormat(t *testing.T) {
	_, err := EncodeFrame(MsgTypeRequest, map[string]any{}, 0xFF)
	if err == nil {
		t.Error("expected error for unsupported format")
	}
}

func TestEncodeDecodeFrame_Roundtrip(t *testing.T) {
	// Test that encoding then decoding preserves the payload
	for _, format := range []byte{FormatJSON, FormatMsgPack} {
		payload := map[string]any{
			"id":           "msg_test123",
			"message_type": "timer.tick",
			"source":       "timer",
			"timestamp_ms": float64(1234567890),
			"payload":      map[string]any{"count": float64(42)},
		}

		frame, err := EncodeFrame(MsgTypeRequest, payload, format)
		if err != nil {
			t.Fatalf("encode error (format %d): %v", format, err)
		}

		decoded, err := TryDecodeFrame(frame)
		if err != nil {
			t.Fatalf("decode error (format %d): %v", format, err)
		}

		decodedMap, ok := decoded.Payload.(map[string]any)
		if !ok {
			t.Fatalf("expected map payload (format %d)", format)
		}
		if decodedMap["id"] != "msg_test123" {
			t.Errorf("expected msg_test123, got %v (format %d)", decodedMap["id"], format)
		}
	}
}

func TestMultipleFramesInBuffer(t *testing.T) {
	// Encode two frames and concatenate them
	frame1, _ := EncodeFrame(MsgTypeResponse, map[string]any{"n": float64(1)}, FormatJSON)
	frame2, _ := EncodeFrame(MsgTypeResponse, map[string]any{"n": float64(2)}, FormatJSON)

	buffer := append(frame1, frame2...)

	// Decode first
	decoded1, err := TryDecodeFrame(buffer)
	if err != nil {
		t.Fatalf("error decoding first frame: %v", err)
	}
	if decoded1 == nil {
		t.Fatal("expected first frame")
	}

	// Decode second from remaining buffer
	remaining := buffer[decoded1.BytesConsumed:]
	decoded2, err := TryDecodeFrame(remaining)
	if err != nil {
		t.Fatalf("error decoding second frame: %v", err)
	}
	if decoded2 == nil {
		t.Fatal("expected second frame")
	}

	p1, _ := decoded1.Payload.(map[string]any)
	p2, _ := decoded2.Payload.(map[string]any)
	if p1["n"] != float64(1) {
		t.Errorf("first frame n=%v, want 1", p1["n"])
	}
	if p2["n"] != float64(2) {
		t.Errorf("second frame n=%v, want 2", p2["n"])
	}
}
