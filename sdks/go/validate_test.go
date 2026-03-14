package emergent

import (
	"strings"
	"testing"
)

func TestValidateMessageType_Valid(t *testing.T) {
	valid := []string{
		"timer.tick",
		"user.created",
		"system.started.timer",
		"event-v2",
		"my_event",
		"a",
		"a.b.c.d",
		"hello-world",
		"test123",
		"a1.b2.c3",
	}
	for _, v := range valid {
		if err := ValidateMessageType(v); err != nil {
			t.Errorf("expected %q to be valid, got error: %v", v, err)
		}
	}
}

func TestValidateMessageType_Invalid(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"", "cannot be empty"},
		{".invalid", "cannot start with a dot"},
		{"invalid.", "cannot end with a dot"},
		{"in..valid", "cannot contain consecutive dots"},
		{"INVALID", "only lowercase"},
		{"inValid", "only lowercase"},
		{"has space", "only lowercase"},
		{"has!special", "only lowercase"},
	}
	for _, tt := range tests {
		err := ValidateMessageType(tt.input)
		if err == nil {
			t.Errorf("expected %q to be invalid", tt.input)
			continue
		}
		if !strings.Contains(err.Error(), tt.want) {
			t.Errorf("for %q, expected error containing %q, got: %v", tt.input, tt.want, err)
		}
	}
}

func TestValidatePrimitiveName_Valid(t *testing.T) {
	valid := []string{
		"timer",
		"user_service",
		"email-handler",
		"filter123",
		"a",
		"unknown",
	}
	for _, v := range valid {
		if err := ValidatePrimitiveName(v); err != nil {
			t.Errorf("expected %q to be valid, got error: %v", v, err)
		}
	}
}

func TestValidatePrimitiveName_Invalid(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"", "cannot be empty"},
		{"1invalid", "must start with a lowercase letter"},
		{"Invalid", "must start with a lowercase letter"},
		{"-invalid", "must start with a lowercase letter"},
		{"_invalid", "must start with a lowercase letter"},
		{"has.dot", "only lowercase"},
		{"has space", "only lowercase"},
		{"HAS_UPPER", "must start with a lowercase letter"},
		{strings.Repeat("a", 65), "cannot exceed 64 characters"},
	}
	for _, tt := range tests {
		err := ValidatePrimitiveName(tt.input)
		if err == nil {
			t.Errorf("expected %q to be invalid", tt.input)
			continue
		}
		if !strings.Contains(err.Error(), tt.want) {
			t.Errorf("for %q, expected error containing %q, got: %v", tt.input, tt.want, err)
		}
	}
}

func TestValidatePrimitiveName_MaxLength(t *testing.T) {
	// Exactly 64 characters should be valid
	name := strings.Repeat("a", 64)
	if err := ValidatePrimitiveName(name); err != nil {
		t.Errorf("expected 64-char name to be valid, got: %v", err)
	}

	// 65 characters should be invalid
	name = strings.Repeat("a", 65)
	if err := ValidatePrimitiveName(name); err == nil {
		t.Error("expected 65-char name to be invalid")
	}
}
