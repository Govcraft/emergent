package emergent

import (
	"testing"
	"time"
)

func TestErrorCodes(t *testing.T) {
	tests := []struct {
		err  EmergentError
		code string
	}{
		{&ConnectionError{Msg: "test"}, "CONNECTION_FAILED"},
		{&SocketNotFoundError{Path: "/tmp/test.sock"}, "SOCKET_NOT_FOUND"},
		{&TimeoutError{Msg: "test", Dur: time.Second}, "TIMEOUT"},
		{&ProtocolError{Msg: "test"}, "PROTOCOL_ERROR"},
		{&SubscriptionError{Msg: "test"}, "SUBSCRIPTION_FAILED"},
		{&PublishError{Msg: "test"}, "PUBLISH_FAILED"},
		{&DiscoveryError{Msg: "test"}, "DISCOVERY_FAILED"},
		{&DisposedError{ClientType: "Source"}, "DISPOSED"},
		{&ValidationError{Msg: "test", Field: "field"}, "VALIDATION_ERROR"},
		{&HelperError{Msg: "test"}, "HELPER_ERROR"},
	}

	for _, tt := range tests {
		if tt.err.Code() != tt.code {
			t.Errorf("expected code %s, got %s for %T", tt.code, tt.err.Code(), tt.err)
		}
		// All errors should produce non-empty messages
		if tt.err.Error() == "" {
			t.Errorf("expected non-empty error message for %T", tt.err)
		}
	}
}

func TestErrorMessages(t *testing.T) {
	err := &ConnectionError{Msg: "connection refused"}
	if err.Error() != "connection failed: connection refused" {
		t.Errorf("unexpected message: %s", err.Error())
	}

	err2 := &SocketNotFoundError{Path: "/tmp/test.sock"}
	if err2.Error() != "engine socket not found: /tmp/test.sock" {
		t.Errorf("unexpected message: %s", err2.Error())
	}

	err3 := &DisposedError{ClientType: "EmergentSource"}
	if err3.Error() != "EmergentSource has been disposed" {
		t.Errorf("unexpected message: %s", err3.Error())
	}
}
