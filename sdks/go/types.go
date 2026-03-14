package emergent

import (
	"fmt"
	"strings"
	"time"

	"go.jetify.com/typeid"
)

// MessageId is a unique message identifier using TypeID format (msg_<uuid_v7>).
type MessageId string

// NewMessageId generates a new unique message ID.
func NewMessageId() MessageId {
	tid, err := typeid.WithPrefix("msg")
	if err != nil {
		panic(fmt.Sprintf("emergent: failed to generate message ID: %v", err))
	}
	return MessageId(tid.String())
}

// ParseMessageId validates and parses a message ID string.
func ParseMessageId(s string) (MessageId, error) {
	if !strings.HasPrefix(s, "msg_") {
		return "", &ValidationError{Msg: "must start with 'msg_'", Field: "message_id"}
	}
	return MessageId(s), nil
}

func (id MessageId) String() string { return string(id) }

// CorrelationId is a correlation identifier for request-response tracking (cor_<uuid_v7>).
type CorrelationId string

// NewCorrelationId generates a new unique correlation ID.
func NewCorrelationId() CorrelationId {
	tid, err := typeid.WithPrefix("cor")
	if err != nil {
		panic(fmt.Sprintf("emergent: failed to generate correlation ID: %v", err))
	}
	return CorrelationId(tid.String())
}

// ParseCorrelationId validates and parses a correlation ID string.
func ParseCorrelationId(s string) (CorrelationId, error) {
	if !strings.HasPrefix(s, "cor_") {
		return "", &ValidationError{Msg: "must start with 'cor_'", Field: "correlation_id"}
	}
	return CorrelationId(s), nil
}

func (id CorrelationId) String() string { return string(id) }

// CausationId tracks which message triggered this one (cau_<uuid_v7> or msg_<uuid_v7>).
type CausationId string

// NewCausationId generates a new unique causation ID.
func NewCausationId() CausationId {
	tid, err := typeid.WithPrefix("cau")
	if err != nil {
		panic(fmt.Sprintf("emergent: failed to generate causation ID: %v", err))
	}
	return CausationId(tid.String())
}

// ParseCausationId validates and parses a causation ID string.
// Accepts both "cau_" and "msg_" prefixes.
func ParseCausationId(s string) (CausationId, error) {
	if !strings.HasPrefix(s, "cau_") && !strings.HasPrefix(s, "msg_") {
		return "", &ValidationError{Msg: "must start with 'cau_' or 'msg_'", Field: "causation_id"}
	}
	return CausationId(s), nil
}

// CausationIdFromMessage converts a MessageId to a CausationId.
func CausationIdFromMessage(msgID MessageId) CausationId {
	return CausationId(msgID)
}

func (id CausationId) String() string { return string(id) }

// MessageType is a validated message type identifier (e.g., "timer.tick").
type MessageType string

// NewMessageType creates a validated MessageType.
func NewMessageType(value string) (MessageType, error) {
	if err := ValidateMessageType(value); err != nil {
		return "", err
	}
	return MessageType(value), nil
}

func (mt MessageType) String() string { return string(mt) }

// Parts splits the message type by dots.
func (mt MessageType) Parts() []string { return strings.Split(string(mt), ".") }

// Category returns the first part of the message type (before the first dot).
func (mt MessageType) Category() string {
	parts := mt.Parts()
	if len(parts) > 0 {
		return parts[0]
	}
	return ""
}

// MatchesPattern checks if this message type matches a pattern.
// Supports exact match and wildcard patterns (e.g., "timer.*").
func (mt MessageType) MatchesPattern(pattern string) bool {
	if pattern == string(mt) {
		return true
	}
	if strings.HasSuffix(pattern, ".*") {
		prefix := strings.TrimSuffix(pattern, ".*")
		return strings.HasPrefix(string(mt), prefix+".")
	}
	return false
}

// PrimitiveName is a validated primitive name identifier.
type PrimitiveName string

// NewPrimitiveName creates a validated PrimitiveName.
func NewPrimitiveName(value string) (PrimitiveName, error) {
	if err := ValidatePrimitiveName(value); err != nil {
		return "", err
	}
	return PrimitiveName(value), nil
}

func (pn PrimitiveName) String() string { return string(pn) }

// IsDefault returns true if the name is the default "unknown" value.
func (pn PrimitiveName) IsDefault() bool { return pn == "unknown" }

// Timestamp is milliseconds since the Unix epoch.
type Timestamp uint64

// TimestampNow returns the current time as a Timestamp.
func TimestampNow() Timestamp { return Timestamp(time.Now().UnixMilli()) }

// TimestampFromMillis creates a Timestamp from milliseconds.
func TimestampFromMillis(ms uint64) Timestamp { return Timestamp(ms) }

// AsMillis returns the timestamp as milliseconds.
func (ts Timestamp) AsMillis() uint64 { return uint64(ts) }

// AsSecs returns the timestamp as truncated seconds.
func (ts Timestamp) AsSecs() uint64 { return uint64(ts) / 1000 }

// PrimitiveKind represents the type of primitive.
type PrimitiveKind string

const (
	PrimitiveKindSource  PrimitiveKind = "Source"
	PrimitiveKindHandler PrimitiveKind = "Handler"
	PrimitiveKindSink    PrimitiveKind = "Sink"
)

// PrimitiveInfo describes a registered primitive.
type PrimitiveInfo struct {
	Name string        `json:"name" msgpack:"name"`
	Kind PrimitiveKind `json:"kind" msgpack:"kind"`
}

// DiscoveryInfo contains discovery information from the engine.
type DiscoveryInfo struct {
	MessageTypes []string        `json:"message_types" msgpack:"message_types"`
	Primitives   []PrimitiveInfo `json:"primitives" msgpack:"primitives"`
}

// ConnectOptions configures connection parameters.
type ConnectOptions struct {
	// SocketPath overrides the EMERGENT_SOCKET environment variable.
	SocketPath string
	// Timeout for connection and request operations (default: 30s).
	Timeout time.Duration
}

// TopologyPrimitive describes a primitive in the topology.
type TopologyPrimitive struct {
	Name       string   `json:"name" msgpack:"name"`
	Kind       string   `json:"kind" msgpack:"kind"`
	State      string   `json:"state" msgpack:"state"`
	Publishes  []string `json:"publishes,omitempty" msgpack:"publishes,omitempty"`
	Subscribes []string `json:"subscribes,omitempty" msgpack:"subscribes,omitempty"`
	PID        *uint32  `json:"pid,omitempty" msgpack:"pid,omitempty"`
	Error      *string  `json:"error,omitempty" msgpack:"error,omitempty"`
}

// TopologyState is the current topology of all primitives.
type TopologyState struct {
	Primitives []TopologyPrimitive `json:"primitives" msgpack:"primitives"`
}

// generateCorrelationID generates a TypeID with the given prefix.
func generateCorrelationID(prefix string) string {
	tid, err := typeid.WithPrefix(prefix)
	if err != nil {
		panic(fmt.Sprintf("emergent: failed to generate correlation ID with prefix %q: %v", prefix, err))
	}
	return tid.String()
}
