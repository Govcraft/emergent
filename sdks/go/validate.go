package emergent

import (
	"strings"
	"unicode"
)

// MaxPrimitiveNameLength is the maximum allowed length for a primitive name.
const MaxPrimitiveNameLength = 64

// ValidateMessageType validates a message type string.
//
// Rules:
//   - Cannot be empty
//   - Only lowercase letters, digits, dots (.), hyphens (-), underscores (_)
//   - Cannot start or end with a dot
//   - Cannot have consecutive dots
func ValidateMessageType(value string) error {
	if value == "" {
		return &ValidationError{Msg: "cannot be empty", Field: "message_type"}
	}

	if strings.HasPrefix(value, ".") {
		return &ValidationError{Msg: "cannot start with a dot", Field: "message_type"}
	}

	if strings.HasSuffix(value, ".") {
		return &ValidationError{Msg: "cannot end with a dot", Field: "message_type"}
	}

	if strings.Contains(value, "..") {
		return &ValidationError{Msg: "cannot contain consecutive dots", Field: "message_type"}
	}

	for _, r := range value {
		if !(unicode.IsLower(r) || unicode.IsDigit(r) || r == '.' || r == '-' || r == '_') {
			return &ValidationError{
				Msg:   "only lowercase letters, digits, dots, hyphens, and underscores are allowed",
				Field: "message_type",
			}
		}
	}

	return nil
}

// ValidatePrimitiveName validates a primitive name string.
//
// Rules:
//   - Cannot be empty
//   - Must start with a lowercase letter
//   - Only lowercase letters, digits, hyphens (-), underscores (_)
//   - Cannot exceed 64 characters
func ValidatePrimitiveName(value string) error {
	if value == "" {
		return &ValidationError{Msg: "cannot be empty", Field: "primitive_name"}
	}

	if len(value) > MaxPrimitiveNameLength {
		return &ValidationError{
			Msg:   "cannot exceed 64 characters",
			Field: "primitive_name",
		}
	}

	first := rune(value[0])
	if !unicode.IsLower(first) || !unicode.IsLetter(first) {
		return &ValidationError{
			Msg:   "must start with a lowercase letter",
			Field: "primitive_name",
		}
	}

	for _, r := range value {
		if !(unicode.IsLower(r) || unicode.IsDigit(r) || r == '-' || r == '_') {
			return &ValidationError{
				Msg:   "only lowercase letters, digits, hyphens, and underscores are allowed",
				Field: "primitive_name",
			}
		}
	}

	return nil
}
