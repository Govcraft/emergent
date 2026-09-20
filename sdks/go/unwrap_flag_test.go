package emergent

import "testing"

// The same table runs in the Rust, Python and TypeScript SDKs.
func TestParseUnwrapFlag(t *testing.T) {
	tests := []struct {
		value string
		want  bool
	}{
		{"true", true},
		{"1", true},
		{"TRUE", true},
		{"True", true},
		{" true ", true},
		{" 1 ", true},
		{"\ttrue\n", true},
		// An unset variable reads as the empty string.
		{"", false},
		{" ", false},
		{"false", false},
		{"0", false},
		{"no", false},
		{"off", false},
		{"yes", false},
		{"on", false},
		{"2", false},
		{"11", false},
		{"truee", false},
	}
	for _, tt := range tests {
		t.Run(tt.value, func(t *testing.T) {
			if got := parseUnwrapFlag(tt.value); got != tt.want {
				t.Errorf("parseUnwrapFlag(%q) = %v, want %v", tt.value, got, tt.want)
			}
		})
	}
}

func TestNewBaseClientReadsTheUnwrapFlag(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")
	t.Setenv("EMERGENT_UNWRAP_STDOUT", " TRUE ")
	if client := newBaseClient("unit", PrimitiveKindSink, nil); !client.unwrapStdout {
		t.Error("unwrapStdout = false, want true for \" TRUE \"")
	}
}
