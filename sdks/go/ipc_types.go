package emergent

// IpcEnvelope is the request envelope sent to the engine.
type IpcEnvelope struct {
	CorrelationID string `json:"correlation_id" msgpack:"correlation_id"`
	Target        string `json:"target" msgpack:"target"`
	MessageType   string `json:"message_type" msgpack:"message_type"`
	Payload       any    `json:"payload" msgpack:"payload"`
	ExpectsReply  bool   `json:"expects_reply" msgpack:"expects_reply"`
}

// IpcResponse is the response from the engine.
type IpcResponse struct {
	CorrelationID string `json:"correlation_id" msgpack:"correlation_id"`
	Success       bool   `json:"success" msgpack:"success"`
	Payload       any    `json:"payload,omitempty" msgpack:"payload,omitempty"`
	Error         string `json:"error,omitempty" msgpack:"error,omitempty"`
	ErrorCode     string `json:"error_code,omitempty" msgpack:"error_code,omitempty"`
}

// IpcSubscribeRequest is the subscription request payload.
type IpcSubscribeRequest struct {
	CorrelationID string   `json:"correlation_id" msgpack:"correlation_id"`
	MessageTypes  []string `json:"message_types" msgpack:"message_types"`
}

// IpcPushNotification is a push notification from subscriptions.
type IpcPushNotification struct {
	NotificationID string `json:"notification_id" msgpack:"notification_id"`
	MessageType    string `json:"message_type" msgpack:"message_type"`
	Payload        any    `json:"payload" msgpack:"payload"`
	SourceActor    string `json:"source_actor,omitempty" msgpack:"source_actor,omitempty"`
	TimestampMs    uint64 `json:"timestamp_ms" msgpack:"timestamp_ms"`
}

// WireMessage is the snake_case wire format of EmergentMessage.
type WireMessage struct {
	ID            string `json:"id" msgpack:"id"`
	MessageType   string `json:"message_type" msgpack:"message_type"`
	Source        string `json:"source" msgpack:"source"`
	CorrelationID string `json:"correlation_id,omitempty" msgpack:"correlation_id,omitempty"`
	CausationID   string `json:"causation_id,omitempty" msgpack:"causation_id,omitempty"`
	TimestampMs   uint64 `json:"timestamp_ms" msgpack:"timestamp_ms"`
	Payload       any    `json:"payload" msgpack:"payload"`
	Metadata      any    `json:"metadata,omitempty" msgpack:"metadata,omitempty"`
}

// IpcDiscoverResponse is the response from a discovery request.
type IpcDiscoverResponse struct {
	MessageTypes []string            `json:"message_types" msgpack:"message_types"`
	Primitives   []map[string]string `json:"primitives" msgpack:"primitives"`
}
