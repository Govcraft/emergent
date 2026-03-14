package emergent

import (
	"strings"
	"testing"
)

func TestNewMessage(t *testing.T) {
	msg, err := NewMessage("timer.tick")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if !strings.HasPrefix(string(msg.ID), "msg_") {
		t.Errorf("expected msg_ prefix, got: %s", msg.ID)
	}
	if msg.MessageType != "timer.tick" {
		t.Errorf("expected timer.tick, got: %s", msg.MessageType)
	}
	if msg.Source != "unknown" {
		t.Errorf("expected unknown source, got: %s", msg.Source)
	}
	if msg.TimestampMs == 0 {
		t.Error("expected non-zero timestamp")
	}
}

func TestNewMessage_InvalidType(t *testing.T) {
	_, err := NewMessage("INVALID")
	if err == nil {
		t.Error("expected error for invalid message type")
	}
}

func TestMessage_WithPayload(t *testing.T) {
	msg, _ := NewMessage("test.event")
	result := msg.WithPayload(map[string]any{"count": 42})

	// Should return same pointer for chaining
	if result != msg {
		t.Error("WithPayload should return same pointer")
	}

	payload, ok := msg.Payload.(map[string]any)
	if !ok {
		t.Fatal("expected map payload")
	}
	if payload["count"] != 42 {
		t.Errorf("expected count=42, got: %v", payload["count"])
	}
}

func TestMessage_WithSource(t *testing.T) {
	msg, _ := NewMessage("test.event")
	msg.WithSource("my_source")

	if msg.Source != "my_source" {
		t.Errorf("expected my_source, got: %s", msg.Source)
	}
}

func TestMessage_WithCorrelationID(t *testing.T) {
	msg, _ := NewMessage("test.event")
	msg.WithCorrelationID("cor_abc123")

	if msg.CorrelationID != "cor_abc123" {
		t.Errorf("expected cor_abc123, got: %s", msg.CorrelationID)
	}
}

func TestMessage_WithCausationFromMessage(t *testing.T) {
	msg, _ := NewMessage("test.event")
	parentID := MessageId("msg_parent123")
	msg.WithCausationFromMessage(parentID)

	if msg.CausationID != "msg_parent123" {
		t.Errorf("expected msg_parent123, got: %s", msg.CausationID)
	}
}

func TestMessage_WithMetadata(t *testing.T) {
	msg, _ := NewMessage("test.event")
	msg.WithMetadata(map[string]any{"trace_id": "abc"})

	meta, ok := msg.Metadata.(map[string]any)
	if !ok {
		t.Fatal("expected map metadata")
	}
	if meta["trace_id"] != "abc" {
		t.Errorf("expected trace_id=abc, got: %v", meta["trace_id"])
	}
}

func TestMessage_Chaining(t *testing.T) {
	msg, _ := NewMessage("test.event")
	msg.WithSource("src").
		WithPayload(map[string]any{"key": "value"}).
		WithCorrelationID("cor_123").
		WithMetadata(map[string]any{"debug": true})

	if msg.Source != "src" {
		t.Errorf("source not set")
	}
	if msg.CorrelationID != "cor_123" {
		t.Errorf("correlation_id not set")
	}
}

func TestMessage_PayloadAs(t *testing.T) {
	msg, _ := NewMessage("test.event")
	msg.WithPayload(map[string]any{"count": float64(42), "name": "test"})

	type MyPayload struct {
		Count float64 `json:"count"`
		Name  string  `json:"name"`
	}

	var p MyPayload
	if err := msg.PayloadAs(&p); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if p.Count != 42 {
		t.Errorf("expected count=42, got: %v", p.Count)
	}
	if p.Name != "test" {
		t.Errorf("expected name=test, got: %v", p.Name)
	}
}

func TestMessage_ToWire(t *testing.T) {
	msg, _ := NewMessage("timer.tick")
	msg.WithSource("timer").
		WithPayload(map[string]any{"count": 1}).
		WithCorrelationID("cor_abc").
		WithCausationID("msg_parent")

	wire := msg.ToWire()

	if wire["message_type"] != "timer.tick" {
		t.Errorf("unexpected message_type: %v", wire["message_type"])
	}
	if wire["source"] != "timer" {
		t.Errorf("unexpected source: %v", wire["source"])
	}
	if wire["correlation_id"] != "cor_abc" {
		t.Errorf("unexpected correlation_id: %v", wire["correlation_id"])
	}
	if wire["causation_id"] != "msg_parent" {
		t.Errorf("unexpected causation_id: %v", wire["causation_id"])
	}
}

func TestMessage_ToWire_OmitsEmpty(t *testing.T) {
	msg, _ := NewMessage("test.event")

	wire := msg.ToWire()

	if _, ok := wire["correlation_id"]; ok {
		t.Error("expected correlation_id to be omitted when empty")
	}
	if _, ok := wire["causation_id"]; ok {
		t.Error("expected causation_id to be omitted when empty")
	}
	if _, ok := wire["metadata"]; ok {
		t.Error("expected metadata to be omitted when nil")
	}
}

func TestMessageFromWire(t *testing.T) {
	wire := map[string]any{
		"id":             "msg_abc123",
		"message_type":   "timer.tick",
		"source":         "timer",
		"correlation_id": "cor_def456",
		"causation_id":   "msg_parent",
		"timestamp_ms":   uint64(1234567890123),
		"payload":        map[string]any{"count": 42},
		"metadata":       map[string]any{"debug": true},
	}

	msg := MessageFromWire(wire)

	if string(msg.ID) != "msg_abc123" {
		t.Errorf("unexpected id: %s", msg.ID)
	}
	if string(msg.MessageType) != "timer.tick" {
		t.Errorf("unexpected message_type: %s", msg.MessageType)
	}
	if string(msg.Source) != "timer" {
		t.Errorf("unexpected source: %s", msg.Source)
	}
	if msg.CorrelationID != "cor_def456" {
		t.Errorf("unexpected correlation_id: %s", msg.CorrelationID)
	}
	if msg.CausationID != "msg_parent" {
		t.Errorf("unexpected causation_id: %s", msg.CausationID)
	}
	if msg.TimestampMs != 1234567890123 {
		t.Errorf("unexpected timestamp_ms: %d", msg.TimestampMs)
	}
}

func TestMessageFromWire_Float64Timestamp(t *testing.T) {
	// JSON deserialization produces float64 for numbers
	wire := map[string]any{
		"id":           "msg_abc",
		"message_type": "test",
		"source":       "src",
		"timestamp_ms": float64(1234567890123),
		"payload":      nil,
	}

	msg := MessageFromWire(wire)
	if msg.TimestampMs != 1234567890123 {
		t.Errorf("unexpected timestamp: %d", msg.TimestampMs)
	}
}
