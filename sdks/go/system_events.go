package emergent

// SystemEventPayload is the payload for system.started.*, system.stopped.*, and system.error.* events.
type SystemEventPayload struct {
	Name       string   `json:"name" msgpack:"name"`
	Kind       string   `json:"kind" msgpack:"kind"`
	PID        *uint32  `json:"pid,omitempty" msgpack:"pid,omitempty"`
	Publishes  []string `json:"publishes,omitempty" msgpack:"publishes,omitempty"`
	Subscribes []string `json:"subscribes,omitempty" msgpack:"subscribes,omitempty"`
	Error      *string  `json:"error,omitempty" msgpack:"error,omitempty"`
}

// IsSource returns true if the primitive is a source.
func (p *SystemEventPayload) IsSource() bool { return p.Kind == "source" }

// IsHandler returns true if the primitive is a handler.
func (p *SystemEventPayload) IsHandler() bool { return p.Kind == "handler" }

// IsSink returns true if the primitive is a sink.
func (p *SystemEventPayload) IsSink() bool { return p.Kind == "sink" }

// IsError returns true if this is an error event.
func (p *SystemEventPayload) IsError() bool { return p.Error != nil }

// SystemShutdownPayload is the payload for system.shutdown events.
type SystemShutdownPayload struct {
	Kind string `json:"kind" msgpack:"kind"`
}

// IsSourceShutdown returns true if this shutdown targets sources.
func (p *SystemShutdownPayload) IsSourceShutdown() bool { return p.Kind == "source" }

// IsHandlerShutdown returns true if this shutdown targets handlers.
func (p *SystemShutdownPayload) IsHandlerShutdown() bool { return p.Kind == "handler" }

// IsSinkShutdown returns true if this shutdown targets sinks.
func (p *SystemShutdownPayload) IsSinkShutdown() bool { return p.Kind == "sink" }
