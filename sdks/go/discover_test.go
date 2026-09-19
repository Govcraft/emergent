package emergent

import (
	"context"
	"reflect"
	"testing"
	"time"

	"github.com/vmihailenco/msgpack/v5"
)

// engineDiscoveryBody is a discovery reply captured from a real engine. The
// lists sit at the top level of the body. There is no payload key and no
// primitives key.
func engineDiscoveryBody(correlationID string) map[string]any {
	return map[string]any{
		"correlation_id": correlationID,
		"success":        true,
		"protocol_version": map[string]any{
			"current":       2,
			"min_supported": 1,
			"max_supported": 2,
			"description":   "v2 (multi-format, streaming, push, discovery)",
			"capabilities": map[string]any{
				"messagepack": true,
				"streaming":   true,
				"push":        true,
				"discovery":   true,
			},
		},
		"actors": []any{
			map[string]any{
				"name": "message_broker",
				"ern":  "ern:acton:reactive:component:message_broker_01m2xw6x0jfhsrmbxrg90t657d",
			},
		},
		"message_types": []any{"SystemEvent", "EmergentMessage"},
	}
}

// overWire returns the body as the client sees it: encoded and decoded again
// through MessagePack.
func overWire(t *testing.T, body map[string]any) map[string]any {
	t.Helper()
	encoded, err := msgpack.Marshal(body)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	var decoded map[string]any
	if err := msgpack.Unmarshal(encoded, &decoded); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	return decoded
}

func TestDiscoveryInfoFromBody(t *testing.T) {
	without := func(key string) map[string]any {
		body := engineDiscoveryBody("disc_1")
		delete(body, key)
		return body
	}

	tests := []struct {
		name string
		body map[string]any
		want *DiscoveryInfo
	}{
		{
			"the engine's reply",
			engineDiscoveryBody("disc_1"),
			&DiscoveryInfo{
				MessageTypes: []string{"SystemEvent", "EmergentMessage"},
				Primitives:   []PrimitiveInfo{{Name: "message_broker"}},
			},
		},
		{
			"actors left out",
			without("actors"),
			&DiscoveryInfo{MessageTypes: []string{"SystemEvent", "EmergentMessage"}},
		},
		{
			"message types left out",
			without("message_types"),
			&DiscoveryInfo{Primitives: []PrimitiveInfo{{Name: "message_broker"}}},
		},
		{
			"entries of the wrong shape are skipped",
			map[string]any{
				"actors":        []any{"message_broker", map[string]any{"ern": "no name"}, map[string]any{"name": "broker"}},
				"message_types": []any{"SystemEvent", 7},
			},
			&DiscoveryInfo{MessageTypes: []string{"SystemEvent"}, Primitives: []PrimitiveInfo{{Name: "broker"}}},
		},
		{
			"lists under payload are not read",
			map[string]any{"payload": map[string]any{"message_types": []any{"SystemEvent"}, "primitives": []any{}}},
			&DiscoveryInfo{},
		},
		{"nil body", nil, &DiscoveryInfo{}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := discoveryInfoFromBody(overWire(t, tt.body)); !reflect.DeepEqual(got, tt.want) {
				t.Errorf("discoveryInfoFromBody() = %+v, want %+v", got, tt.want)
			}
		})
	}
}

// TestDiscoveryReportsNoKind pins what the reply cannot tell: the engine
// names IPC actors and sends no kind for them.
func TestDiscoveryReportsNoKind(t *testing.T) {
	info := discoveryInfoFromBody(overWire(t, engineDiscoveryBody("disc_1")))
	if len(info.Primitives) != 1 || info.Primitives[0].Kind != "" {
		t.Errorf("unexpected primitives: %+v", info.Primitives)
	}
}

// TestIpcDiscoverResponseDecodesTheEngineReply keeps the exported wire type in
// step with the body the engine sends.
func TestIpcDiscoverResponseDecodesTheEngineReply(t *testing.T) {
	encoded, err := msgpack.Marshal(engineDiscoveryBody("disc_1"))
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	var reply IpcDiscoverResponse
	if err := msgpack.Unmarshal(encoded, &reply); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}

	if reply.CorrelationID != "disc_1" || !reply.Success {
		t.Errorf("unexpected header fields: %+v", reply)
	}
	if !reflect.DeepEqual(reply.MessageTypes, []string{"SystemEvent", "EmergentMessage"}) {
		t.Errorf("MessageTypes = %v", reply.MessageTypes)
	}
	if len(reply.Actors) != 1 || reply.Actors[0].Name != "message_broker" || reply.Actors[0].Ern == "" {
		t.Errorf("Actors = %+v", reply.Actors)
	}
	if reply.ProtocolVersion["description"] == nil {
		t.Errorf("ProtocolVersion = %v", reply.ProtocolVersion)
	}
}

func TestDiscoverRequestBody(t *testing.T) {
	frame, err := EncodeFrame(MsgTypeDiscover, &IpcDiscoverRequest{
		CorrelationID: "req_1", IncludeActors: true, IncludeMessageTypes: true,
	}, FormatMsgPack)
	if err != nil {
		t.Fatalf("encode failed: %v", err)
	}
	decoded, err := TryDecodeFrame(frame)
	if err != nil || decoded == nil {
		t.Fatalf("decode failed: %v", err)
	}

	want := map[string]any{"correlation_id": "req_1", "include_actors": true, "include_message_types": true}
	if decoded.MsgType != MsgTypeDiscover || !reflect.DeepEqual(decoded.Payload, want) {
		t.Errorf("frame = type 0x%02x body %v, want type 0x%02x body %v", decoded.MsgType, decoded.Payload, MsgTypeDiscover, want)
	}
}

// TestDiscoverAgainstTheFakeEngine runs the whole call. Every client kind
// shares discoverInternal, so each is checked through its own method.
func TestDiscoverAgainstTheFakeEngine(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	engine := startFakeEngine(t, 0)
	opts := &ConnectOptions{SocketPath: engine.socketPath, Timeout: 5 * time.Second}

	source, err := ConnectSource("discover_source", opts)
	if err != nil {
		t.Fatalf("failed to connect source: %v", err)
	}
	t.Cleanup(func() { _ = source.Close() })
	handler, err := ConnectHandler("discover_handler", opts)
	if err != nil {
		t.Fatalf("failed to connect handler: %v", err)
	}
	t.Cleanup(func() { _ = handler.Close() })
	sink := connectTestSink(t, engine, 5*time.Second)

	clients := []struct {
		name     string
		discover func(context.Context) (*DiscoveryInfo, error)
	}{
		{"source", source.Discover},
		{"handler", handler.Discover},
		{"sink", sink.Discover},
	}

	want := &DiscoveryInfo{
		MessageTypes: []string{"SystemEvent", "EmergentMessage"},
		Primitives:   []PrimitiveInfo{{Name: "message_broker"}},
	}
	for _, client := range clients {
		t.Run(client.name, func(t *testing.T) {
			info, discoverErr := client.discover(context.Background())
			if discoverErr != nil {
				t.Fatalf("Discover failed: %v", discoverErr)
			}
			if !reflect.DeepEqual(info, want) {
				t.Errorf("Discover() = %+v, want %+v", info, want)
			}
		})
	}
}
