package emergent

import (
	"encoding/json"
	"testing"
)

func TestSystemEventPayload_Predicates(t *testing.T) {
	source := &SystemEventPayload{Name: "timer", Kind: "source"}
	if !source.IsSource() {
		t.Error("expected IsSource")
	}
	if source.IsHandler() || source.IsSink() {
		t.Error("should not be handler or sink")
	}

	handler := &SystemEventPayload{Name: "filter", Kind: "handler"}
	if !handler.IsHandler() {
		t.Error("expected IsHandler")
	}

	sink := &SystemEventPayload{Name: "console", Kind: "sink"}
	if !sink.IsSink() {
		t.Error("expected IsSink")
	}
}

func TestSystemEventPayload_IsError(t *testing.T) {
	p := &SystemEventPayload{Name: "timer", Kind: "source"}
	if p.IsError() {
		t.Error("should not be error without error field")
	}

	errMsg := "process crashed"
	p.Error = &errMsg
	if !p.IsError() {
		t.Error("expected IsError with error field")
	}
}

func TestSystemEventPayload_JSON(t *testing.T) {
	data := `{
		"name": "timer",
		"kind": "source",
		"pid": 1234,
		"publishes": ["timer.tick"],
		"subscribes": []
	}`

	var p SystemEventPayload
	if err := json.Unmarshal([]byte(data), &p); err != nil {
		t.Fatalf("unmarshal error: %v", err)
	}
	if p.Name != "timer" {
		t.Errorf("expected timer, got: %s", p.Name)
	}
	if p.PID == nil || *p.PID != 1234 {
		t.Errorf("expected pid 1234")
	}
	if len(p.Publishes) != 1 || p.Publishes[0] != "timer.tick" {
		t.Errorf("expected publishes=[timer.tick], got: %v", p.Publishes)
	}
}

func TestSystemShutdownPayload(t *testing.T) {
	handler := &SystemShutdownPayload{Kind: "handler"}
	if !handler.IsHandlerShutdown() {
		t.Error("expected handler shutdown")
	}
	if handler.IsSinkShutdown() {
		t.Error("should not be sink shutdown")
	}

	sink := &SystemShutdownPayload{Kind: "sink"}
	if !sink.IsSinkShutdown() {
		t.Error("expected sink shutdown")
	}
	if sink.IsHandlerShutdown() {
		t.Error("should not be handler shutdown")
	}
}

func TestSystemShutdownPayload_JSON(t *testing.T) {
	data := `{"kind": "handler"}`

	var p SystemShutdownPayload
	if err := json.Unmarshal([]byte(data), &p); err != nil {
		t.Fatalf("unmarshal error: %v", err)
	}
	if p.Kind != "handler" {
		t.Errorf("expected handler, got: %s", p.Kind)
	}
}

// engineShutdownEnvelope is the notification payload the engine sends for a
// system.shutdown broadcast: an EmergentMessage envelope with the kind nested
// in its own payload.
func engineShutdownEnvelope(kind any) map[string]any {
	return map[string]any{
		"id":           "msg_01m2xskqtyffgve4n1yw9vr8kz",
		"message_type": "system.shutdown",
		"source":       "emergent",
		"timestamp_ms": uint64(1758318000000),
		"payload":      map[string]any{"kind": kind},
	}
}

func TestExtractShutdownKind(t *testing.T) {
	tests := []struct {
		name      string
		payload   any
		wantKind  string
		wantFound bool
	}{
		{"engine envelope", engineShutdownEnvelope("sink"), "sink", true},
		{"bare kind object", map[string]any{"kind": "handler"}, "handler", true},
		{"kind is lowercased", engineShutdownEnvelope("Sink"), "sink", true},
		{
			"nested kind wins over an envelope-level kind",
			map[string]any{"kind": "source", "payload": map[string]any{"kind": "sink"}},
			"sink", true,
		},
		{
			"falls back to the envelope when the nested payload has no kind",
			map[string]any{"kind": "source", "payload": map[string]any{"other": 1}},
			"source", true,
		},
		{"non-string kind", engineShutdownEnvelope(42), "", false},
		{"envelope without a kind", map[string]any{"payload": map[string]any{}}, "", false},
		{"nil payload inside the envelope", map[string]any{"payload": nil}, "", false},
		{"nil", nil, "", false},
		{"not a map", "sink", "", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			kind, found := ExtractShutdownKind(tt.payload)
			if kind != tt.wantKind || found != tt.wantFound {
				t.Errorf("ExtractShutdownKind() = (%q, %v), want (%q, %v)", kind, found, tt.wantKind, tt.wantFound)
			}
		})
	}
}
