package emergent

import (
	"strconv"
	"strings"
	"testing"
)

func TestATopicWithoutAStarIsExact(t *testing.T) {
	for _, topic := range []string{"tick.out", "system.shutdown"} {
		kind, err := ClassifyTopic(topic)
		if err != nil {
			t.Fatalf("ClassifyTopic(%q) returned %v", topic, err)
		}
		if kind != TopicExact {
			t.Errorf("ClassifyTopic(%q) = %q, want %q", topic, kind, TopicExact)
		}
	}
}

func TestATerminalStarIsAPattern(t *testing.T) {
	for _, topic := range []string{"tick.*", "system.started.*", "*", "tick*"} {
		kind, err := ClassifyTopic(topic)
		if err != nil {
			t.Fatalf("ClassifyTopic(%q) returned %v", topic, err)
		}
		if kind != TopicPattern {
			t.Errorf("ClassifyTopic(%q) = %q, want %q", topic, kind, TopicPattern)
		}
	}
}

func TestAStarAnywhereElseIsRejected(t *testing.T) {
	for _, topic := range []string{"*.out", "tick.*.out", "tick.**"} {
		_, err := ClassifyTopic(topic)
		if err == nil {
			t.Fatalf("ClassifyTopic(%q) accepted a topic that can never match", topic)
		}
		if !strings.Contains(err.Error(), topic) {
			t.Errorf("error for %q does not name it: %v", topic, err)
		}
	}
}

func TestTheRejectionMessageNamesAUsableSelector(t *testing.T) {
	_, err := ClassifyTopic("system.*.error")
	if err == nil {
		t.Fatal("expected an error")
	}
	if !strings.Contains(err.Error(), `"system.*"`) {
		t.Errorf("error does not suggest a usable selector: %v", err)
	}
}

func TestAnEmptyTopicIsRejected(t *testing.T) {
	if _, err := ClassifyTopic(""); err == nil {
		t.Fatal("expected an error for an empty topic")
	}
}

func TestAnOversizedPatternIsRejected(t *testing.T) {
	topic := strings.Repeat("a", MaxPatternLen) + "*"
	_, err := ClassifyTopic(topic)
	if err == nil {
		t.Fatal("expected an error for an oversized pattern")
	}
	if !strings.Contains(err.Error(), strconv.Itoa(MaxPatternLen)) {
		t.Errorf("error does not name the limit: %v", err)
	}
}

func TestAPatternExactlyAtTheLimitIsAccepted(t *testing.T) {
	topic := strings.Repeat("a", MaxPatternLen-1) + "*"
	kind, err := ClassifyTopic(topic)
	if err != nil {
		t.Fatalf("ClassifyTopic returned %v", err)
	}
	if kind != TopicPattern {
		t.Errorf("got %q, want %q", kind, TopicPattern)
	}
}

func TestPatternPrefixStripsOnlyATerminalStar(t *testing.T) {
	cases := []struct {
		topic  string
		prefix string
		ok     bool
	}{
		{"tick.*", "tick.", true},
		{"*", "", true},
		{"tick.out", "", false},
		{"*.out", "", false},
		{"a*b*", "", false},
	}
	for _, c := range cases {
		prefix, ok := PatternPrefix(c.topic)
		if prefix != c.prefix || ok != c.ok {
			t.Errorf("PatternPrefix(%q) = (%q, %v), want (%q, %v)", c.topic, prefix, ok, c.prefix, c.ok)
		}
	}
}

func TestTopicMatches(t *testing.T) {
	cases := []struct {
		topic       string
		messageType string
		want        bool
	}{
		{"tick.out", "tick.out", true},
		{"tick.out", "tick.exit", false},
		{"tick.out", "tick.out.deep", false},
		{"tick.*", "tick.out", true},
		{"tick.*", "tick.exit", true},
		{"tick.*", "tick.out.deep", true},
		{"tick.*", "tick", false},
		{"tick.*", "ticker.out", false},
		{"system.started.*", "system.started.ticker", true},
		{"system.started.*", "system.stopped.ticker", false},
		{"*", "tick.out", true},
		{"*", "system.shutdown", true},
		{"*.out", "tick.out", false},
		{"*.out", "*.out", false},
	}
	for _, c := range cases {
		if got := TopicMatches(c.topic, c.messageType); got != c.want {
			t.Errorf("TopicMatches(%q, %q) = %v, want %v", c.topic, c.messageType, got, c.want)
		}
	}
}

func TestPartitioningKeepsOrderAndSeparatesTheTwoKinds(t *testing.T) {
	exact, patterns, err := PartitionTopics([]string{"tick.out", "tick.*", "system.started.*", "a"})
	if err != nil {
		t.Fatalf("PartitionTopics returned %v", err)
	}
	if strings.Join(exact, ",") != "tick.out,a" {
		t.Errorf("exact = %v", exact)
	}
	if strings.Join(patterns, ",") != "tick.*,system.started.*" {
		t.Errorf("patterns = %v", patterns)
	}
}

func TestPartitioningReportsTheFirstUnusableTopic(t *testing.T) {
	_, _, err := PartitionTopics([]string{"tick.out", "a*b", "c*d"})
	if err == nil {
		t.Fatal("expected an error")
	}
	if !strings.Contains(err.Error(), "a*b") {
		t.Errorf("error does not name the first unusable topic: %v", err)
	}
}

func TestPartitioningKeepsAnOverlappingPair(t *testing.T) {
	exact, patterns, err := PartitionTopics([]string{"tick.out", "tick.*"})
	if err != nil {
		t.Fatalf("PartitionTopics returned %v", err)
	}
	if len(exact) != 1 || exact[0] != "tick.out" {
		t.Errorf("exact = %v", exact)
	}
	if len(patterns) != 1 || patterns[0] != "tick.*" {
		t.Errorf("patterns = %v", patterns)
	}
}

func TestIsEmergentMessageType(t *testing.T) {
	cases := map[string]bool{
		"tick.out":                   true,
		"system.started.ticker":      true,
		"with-hyphen_and_underscore": true,
		"SystemEvent":                false,
		"EmergentMessage":            false,
		"":                           false,
		"tick..out":                  false,
		".tick":                      false,
	}
	for messageType, want := range cases {
		if got := IsEmergentMessageType(messageType); got != want {
			t.Errorf("IsEmergentMessageType(%q) = %v, want %v", messageType, got, want)
		}
	}
}
