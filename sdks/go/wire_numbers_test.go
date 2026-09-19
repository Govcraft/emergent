package emergent

import (
	"math"
	"testing"

	"github.com/vmihailenco/msgpack/v5"
)

func TestWireUint64(t *testing.T) {
	tests := []struct {
		name   string
		value  any
		want   uint64
		wantOK bool
	}{
		{"uint64", uint64(1758318000000), 1758318000000, true},
		{"uint64 max", uint64(math.MaxUint64), math.MaxUint64, true},
		{"uint32", uint32(70000), 70000, true},
		{"uint16", uint16(4242), 4242, true},
		{"uint8", uint8(200), 200, true},
		{"uint", uint(7), 7, true},
		{"int64", int64(1758318000000), 1758318000000, true},
		{"int32", int32(70000), 70000, true},
		{"int16", int16(4242), 4242, true},
		{"int8", int8(42), 42, true},
		{"int", 7, 7, true},
		{"zero", int8(0), 0, true},
		{"float64", float64(1234567890123), 1234567890123, true},
		{"float64 fraction is truncated", 12.9, 12, true},
		{"float32", float32(4242), 4242, true},
		{"negative int64", int64(-1), 0, false},
		{"negative int8", int8(-1), 0, false},
		{"negative float64", -1.0, 0, false},
		{"NaN", math.NaN(), 0, false},
		{"positive infinity", math.Inf(1), 0, false},
		{"float64 at 2^64", float64(1 << 64), 0, false},
		{"string", "4242", 0, false},
		{"bool", true, 0, false},
		{"nil", nil, 0, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok := wireUint64(tt.value)
			if got != tt.want || ok != tt.wantOK {
				t.Errorf("wireUint64(%v) = (%d, %v), want (%d, %v)", tt.value, got, ok, tt.want, tt.wantOK)
			}
		})
	}
}

func TestWireUint32(t *testing.T) {
	tests := []struct {
		name   string
		value  any
		want   uint32
		wantOK bool
	}{
		{"uint32", uint32(4242), 4242, true},
		{"uint32 max", uint32(math.MaxUint32), math.MaxUint32, true},
		{"float64 from JSON", float64(4242), 4242, true},
		{"int8 from a small MessagePack value", int8(7), 7, true},
		{"one past uint32", uint64(math.MaxUint32) + 1, 0, false},
		{"negative", int32(-4242), 0, false},
		{"nil", nil, 0, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok := wireUint32(tt.value)
			if got != tt.want || ok != tt.wantOK {
				t.Errorf("wireUint32(%v) = (%d, %v), want (%d, %v)", tt.value, got, ok, tt.want, tt.wantOK)
			}
		})
	}
}

// TestWireUint64_MsgPackRoundTrip feeds the helper what the MessagePack decoder
// really produces. The encoder picks the narrowest encoding, so one Go type on
// the way in becomes several on the way out.
func TestWireUint64_MsgPackRoundTrip(t *testing.T) {
	values := []uint64{0, 1, 127, 128, 255, 256, 4242, 65535, 65536, 70000, math.MaxUint32, math.MaxUint32 + 1, 1758318000000}

	for _, want := range values {
		encoded, err := msgpack.Marshal(map[string]any{"n": want})
		if err != nil {
			t.Fatalf("marshal %d: %v", want, err)
		}
		var decoded any
		if err := msgpack.Unmarshal(encoded, &decoded); err != nil {
			t.Fatalf("unmarshal %d: %v", want, err)
		}
		fields, _ := decoded.(map[string]any)

		got, ok := wireUint64(fields["n"])
		if !ok || got != want {
			t.Errorf("wireUint64(%T(%v)) = (%d, %v), want (%d, true)", fields["n"], fields["n"], got, ok, want)
		}
	}
}
