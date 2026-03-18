package emergent

import (
	"encoding/json"
)

// EmergentMessage is the universal message envelope for all Emergent communications.
type EmergentMessage struct {
	// ID is the unique message identifier (TypeID: msg_<uuid_v7>).
	ID MessageId `json:"id" msgpack:"id"`
	// MessageType is the message type for routing (e.g., "timer.tick").
	MessageType MessageType `json:"message_type" msgpack:"message_type"`
	// Source is the primitive that published this message.
	Source PrimitiveName `json:"source" msgpack:"source"`
	// CorrelationID is an optional correlation identifier for request-response patterns.
	CorrelationID string `json:"correlation_id,omitempty" msgpack:"correlation_id,omitempty"`
	// CausationID is an optional identifier of the message that triggered this one.
	CausationID string `json:"causation_id,omitempty" msgpack:"causation_id,omitempty"`
	// TimestampMs is the creation time in Unix milliseconds.
	TimestampMs Timestamp `json:"timestamp_ms" msgpack:"timestamp_ms"`
	// Payload is the user-defined message data.
	Payload any `json:"payload" msgpack:"payload"`
	// Metadata is optional debugging/tracing metadata.
	Metadata any `json:"metadata,omitempty" msgpack:"metadata,omitempty"`
}

// NewMessage creates a new EmergentMessage with the given type.
// Generates a unique ID and sets the timestamp to now.
func NewMessage(messageType string) (*EmergentMessage, error) {
	mt, err := NewMessageType(messageType)
	if err != nil {
		return nil, err
	}
	return &EmergentMessage{
		ID:          NewMessageId(),
		MessageType: mt,
		Source:      "unknown",
		TimestampMs: TimestampNow(),
	}, nil
}

// WithSource sets the source of this message and returns the message for chaining.
func (m *EmergentMessage) WithSource(source string) *EmergentMessage {
	m.Source = PrimitiveName(source)
	return m
}

// WithPayload sets the payload and returns the message for chaining.
func (m *EmergentMessage) WithPayload(payload any) *EmergentMessage {
	m.Payload = payload
	return m
}

// WithCorrelationID sets the correlation ID and returns the message for chaining.
func (m *EmergentMessage) WithCorrelationID(id string) *EmergentMessage {
	m.CorrelationID = id
	return m
}

// WithCausationID sets the causation ID and returns the message for chaining.
func (m *EmergentMessage) WithCausationID(id string) *EmergentMessage {
	m.CausationID = id
	return m
}

// WithCausationFromMessage sets the causation ID from a message ID and returns the message for chaining.
func (m *EmergentMessage) WithCausationFromMessage(msgID MessageId) *EmergentMessage {
	m.CausationID = string(msgID)
	return m
}

// WithMetadata sets the metadata and returns the message for chaining.
func (m *EmergentMessage) WithMetadata(metadata any) *EmergentMessage {
	m.Metadata = metadata
	return m
}

// HasStdoutPayload checks whether this message has an exec-source payload shape.
// Returns true if the payload is a map with a "stdout" key that is a string.
func (m *EmergentMessage) HasStdoutPayload() bool {
	payloadMap, ok := m.Payload.(map[string]any)
	if !ok {
		return false
	}
	_, ok = payloadMap["stdout"].(string)
	return ok
}

// UnwrapStdout extracts and parses the stdout field from an exec-source payload.
// If the payload is a map with a "stdout" string key, attempts to parse it as JSON.
// On success, the payload is replaced with the parsed value; on failure, the
// payload is replaced with the raw stdout string.
// If the payload doesn't match the exec-source shape, it is left unchanged.
// Returns self for chaining.
func (m *EmergentMessage) UnwrapStdout() *EmergentMessage {
	payloadMap, ok := m.Payload.(map[string]any)
	if !ok {
		return m
	}
	stdoutStr, ok := payloadMap["stdout"].(string)
	if !ok {
		return m
	}
	var parsed any
	if err := json.Unmarshal([]byte(stdoutStr), &parsed); err != nil {
		m.Payload = stdoutStr
	} else {
		m.Payload = parsed
	}
	return m
}

// PayloadAs unmarshals the payload into the given target.
// Uses JSON re-marshaling to support typed deserialization from map[string]any.
func (m *EmergentMessage) PayloadAs(target any) error {
	data, err := json.Marshal(m.Payload)
	if err != nil {
		return err
	}
	return json.Unmarshal(data, target)
}

// ToWire converts the message to wire format for serialization.
func (m *EmergentMessage) ToWire() map[string]any {
	wire := map[string]any{
		"id":           string(m.ID),
		"message_type": string(m.MessageType),
		"source":       string(m.Source),
		"timestamp_ms": uint64(m.TimestampMs),
		"payload":      m.Payload,
	}
	if m.CorrelationID != "" {
		wire["correlation_id"] = m.CorrelationID
	}
	if m.CausationID != "" {
		wire["causation_id"] = m.CausationID
	}
	if m.Metadata != nil {
		wire["metadata"] = m.Metadata
	}
	return wire
}

// MessageFromWire creates an EmergentMessage from a wire format map.
func MessageFromWire(wire map[string]any) *EmergentMessage {
	msg := &EmergentMessage{}

	if id, ok := wire["id"].(string); ok {
		msg.ID = MessageId(id)
	}
	if mt, ok := wire["message_type"].(string); ok {
		msg.MessageType = MessageType(mt)
	}
	if src, ok := wire["source"].(string); ok {
		msg.Source = PrimitiveName(src)
	}
	if cid, ok := wire["correlation_id"].(string); ok {
		msg.CorrelationID = cid
	}
	if cid, ok := wire["causation_id"].(string); ok {
		msg.CausationID = cid
	}
	if ts, ok := wire["timestamp_ms"].(uint64); ok {
		msg.TimestampMs = Timestamp(ts)
	} else if ts, ok := wire["timestamp_ms"].(int64); ok {
		msg.TimestampMs = Timestamp(ts)
	} else if ts, ok := wire["timestamp_ms"].(float64); ok {
		msg.TimestampMs = Timestamp(ts)
	}
	msg.Payload = wire["payload"]
	msg.Metadata = wire["metadata"]

	return msg
}
