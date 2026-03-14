package emergent

import (
	"fmt"
	"time"
)

// EmergentError is the interface implemented by all SDK error types.
type EmergentError interface {
	error
	Code() string
}

// ConnectionError indicates a failure to connect to the engine.
type ConnectionError struct {
	Msg string
}

func (e *ConnectionError) Error() string { return fmt.Sprintf("connection failed: %s", e.Msg) }
func (e *ConnectionError) Code() string  { return "CONNECTION_FAILED" }

// SocketNotFoundError indicates the engine socket file was not found.
type SocketNotFoundError struct {
	Path string
}

func (e *SocketNotFoundError) Error() string {
	return fmt.Sprintf("engine socket not found: %s", e.Path)
}
func (e *SocketNotFoundError) Code() string { return "SOCKET_NOT_FOUND" }

// TimeoutError indicates an operation exceeded its deadline.
type TimeoutError struct {
	Msg string
	Dur time.Duration
}

func (e *TimeoutError) Error() string {
	return fmt.Sprintf("timeout after %s: %s", e.Dur, e.Msg)
}
func (e *TimeoutError) Code() string { return "TIMEOUT" }

// ProtocolError indicates a wire protocol violation.
type ProtocolError struct {
	Msg string
}

func (e *ProtocolError) Error() string { return fmt.Sprintf("protocol error: %s", e.Msg) }
func (e *ProtocolError) Code() string  { return "PROTOCOL_ERROR" }

// SubscriptionError indicates a subscription operation failed.
type SubscriptionError struct {
	Msg          string
	MessageTypes []string
}

func (e *SubscriptionError) Error() string {
	return fmt.Sprintf("subscription failed: %s", e.Msg)
}
func (e *SubscriptionError) Code() string { return "SUBSCRIPTION_FAILED" }

// PublishError indicates a message publish operation failed.
type PublishError struct {
	Msg         string
	MessageType string
}

func (e *PublishError) Error() string { return fmt.Sprintf("publish failed: %s", e.Msg) }
func (e *PublishError) Code() string  { return "PUBLISH_FAILED" }

// DiscoveryError indicates a discovery request failed.
type DiscoveryError struct {
	Msg string
}

func (e *DiscoveryError) Error() string { return fmt.Sprintf("discovery failed: %s", e.Msg) }
func (e *DiscoveryError) Code() string  { return "DISCOVERY_FAILED" }

// DisposedError indicates an operation was attempted on a closed client.
type DisposedError struct {
	ClientType string
}

func (e *DisposedError) Error() string {
	return fmt.Sprintf("%s has been disposed", e.ClientType)
}
func (e *DisposedError) Code() string { return "DISPOSED" }

// ValidationError indicates a value failed validation.
type ValidationError struct {
	Msg   string
	Field string
}

func (e *ValidationError) Error() string {
	return fmt.Sprintf("validation error on %s: %s", e.Field, e.Msg)
}
func (e *ValidationError) Code() string { return "VALIDATION_ERROR" }

// HelperError indicates an error in a helper function (RunSource, RunHandler, RunSink).
type HelperError struct {
	Msg string
}

func (e *HelperError) Error() string { return fmt.Sprintf("helper error: %s", e.Msg) }
func (e *HelperError) Code() string  { return "HELPER_ERROR" }
