package emergent

import (
	"strings"
	"testing"
)

func TestNewMessageId(t *testing.T) {
	id := NewMessageId()
	if !strings.HasPrefix(string(id), "msg_") {
		t.Errorf("expected msg_ prefix, got: %s", id)
	}
	// Should be unique
	id2 := NewMessageId()
	if id == id2 {
		t.Error("expected unique IDs")
	}
}

func TestParseMessageId(t *testing.T) {
	// Valid
	id, err := ParseMessageId("msg_01h5f6g7h8j9k0l1m2n3o4p5q6")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if id != "msg_01h5f6g7h8j9k0l1m2n3o4p5q6" {
		t.Errorf("unexpected id: %s", id)
	}

	// Invalid prefix
	_, err = ParseMessageId("cor_01h5f6g7h8j9k0l1m2n3o4p5q6")
	if err == nil {
		t.Error("expected error for invalid prefix")
	}
}

func TestNewCorrelationId(t *testing.T) {
	id := NewCorrelationId()
	if !strings.HasPrefix(string(id), "cor_") {
		t.Errorf("expected cor_ prefix, got: %s", id)
	}
}

func TestNewCausationId(t *testing.T) {
	id := NewCausationId()
	if !strings.HasPrefix(string(id), "cau_") {
		t.Errorf("expected cau_ prefix, got: %s", id)
	}
}

func TestParseCausationId(t *testing.T) {
	// cau_ prefix is valid
	_, err := ParseCausationId("cau_01h5f6g7h8j9k0l1m2n3o4p5q6")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// msg_ prefix is also valid (common to use message ID as causation)
	_, err = ParseCausationId("msg_01h5f6g7h8j9k0l1m2n3o4p5q6")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Other prefixes are invalid
	_, err = ParseCausationId("cor_01h5f6g7h8j9k0l1m2n3o4p5q6")
	if err == nil {
		t.Error("expected error for cor_ prefix")
	}
}

func TestCausationIdFromMessage(t *testing.T) {
	msgID := MessageId("msg_01h5f6g7h8j9k0l1m2n3o4p5q6")
	causationID := CausationIdFromMessage(msgID)
	if string(causationID) != string(msgID) {
		t.Errorf("expected %s, got %s", msgID, causationID)
	}
}

func TestMessageType(t *testing.T) {
	mt, err := NewMessageType("timer.tick")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if mt.String() != "timer.tick" {
		t.Errorf("unexpected string: %s", mt)
	}

	parts := mt.Parts()
	if len(parts) != 2 || parts[0] != "timer" || parts[1] != "tick" {
		t.Errorf("unexpected parts: %v", parts)
	}

	if mt.Category() != "timer" {
		t.Errorf("unexpected category: %s", mt.Category())
	}
}

func TestMessageType_MatchesPattern(t *testing.T) {
	mt, _ := NewMessageType("timer.tick")

	if !mt.MatchesPattern("timer.tick") {
		t.Error("expected exact match")
	}
	if !mt.MatchesPattern("timer.*") {
		t.Error("expected wildcard match")
	}
	if mt.MatchesPattern("other.*") {
		t.Error("expected no match for different prefix")
	}
	if mt.MatchesPattern("timer.tock") {
		t.Error("expected no match for different exact type")
	}
}

func TestPrimitiveName(t *testing.T) {
	pn, err := NewPrimitiveName("timer")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if pn.IsDefault() {
		t.Error("'timer' should not be default")
	}

	pn2, err := NewPrimitiveName("unknown")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !pn2.IsDefault() {
		t.Error("'unknown' should be default")
	}
}

func TestTimestamp(t *testing.T) {
	ts := TimestampFromMillis(1234567890123)
	if ts.AsMillis() != 1234567890123 {
		t.Errorf("unexpected millis: %d", ts.AsMillis())
	}
	if ts.AsSecs() != 1234567890 {
		t.Errorf("unexpected secs: %d", ts.AsSecs())
	}

	now := TimestampNow()
	if now.AsMillis() == 0 {
		t.Error("TimestampNow should not be zero")
	}
}

func TestGenerateCorrelationID(t *testing.T) {
	prefixes := []string{"req", "sub", "pub", "cor"}
	for _, prefix := range prefixes {
		id := generateCorrelationID(prefix)
		if !strings.HasPrefix(id, prefix+"_") {
			t.Errorf("expected %s_ prefix, got: %s", prefix, id)
		}
	}

	// Unknown prefix should still work (defaults to req)
	id := generateCorrelationID("unknown")
	if !strings.HasPrefix(id, "unknown_") {
		t.Errorf("expected unknown_ prefix, got: %s", id)
	}
}
