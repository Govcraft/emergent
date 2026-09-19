package emergent

import (
	"context"
	"encoding/json"
	"reflect"
	"testing"
	"time"

	"github.com/vmihailenco/msgpack/v5"
)

func TestTopologyPrimitiveFromWire_PID(t *testing.T) {
	pid := func(n uint32) *uint32 { return &n }

	tests := []struct {
		name string
		raw  any
		want *uint32
	}{
		{"float64 from JSON", float64(4242), pid(4242)},
		{"uint32 from MessagePack", uint32(70000), pid(70000)},
		{"uint16 from MessagePack", uint16(4242), pid(4242)},
		{"int8 from MessagePack", int8(7), pid(7)},
		{"int64", int64(4242), pid(4242)},
		{"absent", nil, nil},
		{"negative", int64(-1), nil},
		{"too large for a PID", uint64(1) << 40, nil},
		{"string", "4242", nil},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			wire := map[string]any{"name": "timer", "kind": "source", "state": "running"}
			if tt.raw != nil {
				wire["pid"] = tt.raw
			}
			got := topologyPrimitiveFromWire(wire).PID
			if !reflect.DeepEqual(got, tt.want) {
				t.Errorf("PID = %v, want %v", derefPID(got), derefPID(tt.want))
			}
		})
	}
}

func derefPID(pid *uint32) any {
	if pid == nil {
		return nil
	}
	return *pid
}

// TestTopologyStateFromWire_BothFormats decodes the same topology through
// each wire format and expects the same result, PID included.
func TestTopologyStateFromWire_BothFormats(t *testing.T) {
	failure := "exited with status 1"
	primitives := []any{
		map[string]any{
			"name": "timer", "kind": "source", "state": "running",
			"publishes": []any{"timer.tick"}, "subscribes": []any{}, "pid": uint32(70000),
		},
		map[string]any{
			"name": "console", "kind": "sink", "state": "failed",
			"publishes": []any{}, "subscribes": []any{"timer.tick", "system.error.*"}, "error": failure,
		},
		"not a primitive",
	}

	timerPID := uint32(70000)
	want := &TopologyState{Primitives: []TopologyPrimitive{
		{Name: "timer", Kind: "source", State: "running", Publishes: []string{"timer.tick"}, PID: &timerPID},
		{Name: "console", Kind: "sink", State: "failed", Subscribes: []string{"timer.tick", "system.error.*"}, Error: &failure},
	}}

	formats := []struct {
		name      string
		marshal   func(any) ([]byte, error)
		unmarshal func([]byte, any) error
	}{
		{"MessagePack", msgpack.Marshal, func(b []byte, v any) error { return msgpack.Unmarshal(b, v) }},
		{"JSON", json.Marshal, json.Unmarshal},
	}

	for _, format := range formats {
		t.Run(format.name, func(t *testing.T) {
			encoded, err := format.marshal(primitives)
			if err != nil {
				t.Fatalf("marshal: %v", err)
			}
			var decoded []any
			if err := format.unmarshal(encoded, &decoded); err != nil {
				t.Fatalf("unmarshal: %v", err)
			}

			got := topologyStateFromWire(decoded)
			if !reflect.DeepEqual(got, want) {
				t.Errorf("topologyStateFromWire() = %+v, want %+v", got, want)
			}
		})
	}
}

// TestGetTopologyReportsPIDOverMsgPack runs the whole lookup against the fake
// engine, which speaks MessagePack the way the real engine does.
func TestGetTopologyReportsPIDOverMsgPack(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	sink := connectTestSink(t, engine, 5*time.Second)

	state, err := sink.GetTopology(context.Background())
	if err != nil {
		t.Fatalf("GetTopology failed: %v", err)
	}
	if len(state.Primitives) != 2 {
		t.Fatalf("expected 2 primitives, got %d", len(state.Primitives))
	}

	timer, console := state.Primitives[0], state.Primitives[1]
	if timer.PID == nil || *timer.PID != fakeTimerPID {
		t.Errorf("timer PID = %v, want %d", derefPID(timer.PID), fakeTimerPID)
	}
	if console.PID != nil {
		t.Errorf("console PID = %d, want none", *console.PID)
	}
}
