package emergent

import (
	"fmt"
	"regexp"
	"strings"
)

// MaxPatternLen is the maximum length, in UTF-8 bytes, of a wildcard
// subscription topic.
const MaxPatternLen = 256

// TopicKind is how the engine will match one requested subscription topic.
//
// A subscription topic is either a literal message type or a prefix selector
// ending in a single "*". "tick.*" selects every message type that starts with
// "tick.", and "*" alone selects every message type the engine publishes,
// including types that appear later in the run. The wildcard is terminal: a
// "*" anywhere else could never match, so it is reported as an error instead
// of being accepted and ignored.
//
// These are the semantics of the engine's IPC prefix subscriptions. Engine
// 0.10.10 and earlier accepted a wildcard topic and delivered nothing.
type TopicKind string

const (
	// TopicExact is a literal message type, delivered on an exact match.
	TopicExact TopicKind = "exact"
	// TopicPattern is a prefix selector ending in a single terminal "*".
	TopicPattern TopicKind = "pattern"
)

// messageTypePattern is a message type the engine can publish: dot-separated,
// lowercase.
var messageTypePattern = regexp.MustCompile(`^[a-z0-9_-]+(\.[a-z0-9_-]+)*$`)

// PatternPrefix returns the literal prefix of a terminal-wildcard topic.
//
// "tick.*" yields ("tick.", true) and "*" yields ("", true). A topic with no
// trailing "*" yields ("", false), and so does one whose prefix contains a
// "*", because that topic is not a usable selector.
func PatternPrefix(topic string) (string, bool) {
	prefix, found := strings.CutSuffix(topic, "*")
	if !found || strings.Contains(prefix, "*") {
		return "", false
	}
	return prefix, true
}

// ClassifyTopic decides whether a topic is a literal message type or a
// wildcard selector.
//
// It returns a *ValidationError when the topic is empty, when a "*" appears
// anywhere but the final position, or when a wildcard topic exceeds
// MaxPatternLen bytes. Each of those can never deliver a message, so the
// caller is told rather than left waiting.
func ClassifyTopic(topic string) (TopicKind, error) {
	if topic == "" {
		return "", &ValidationError{Msg: "subscription topic cannot be empty", Field: "topic"}
	}
	if !strings.Contains(topic, "*") {
		return TopicExact, nil
	}
	if _, ok := PatternPrefix(topic); !ok {
		usable, _, _ := strings.Cut(topic, "*")
		return "", &ValidationError{
			Msg: fmt.Sprintf(
				"subscription topic %q can never match: '*' is only a wildcard as the final "+
					"character, so write a prefix selector such as %q instead",
				topic, usable+"*",
			),
			Field: "topic",
		}
	}
	if len(topic) > MaxPatternLen {
		return "", &ValidationError{
			Msg: fmt.Sprintf(
				"subscription topic %q is %d bytes, over the %d-byte limit for a wildcard topic",
				topic, len(topic), MaxPatternLen,
			),
			Field: "topic",
		}
	}
	return TopicPattern, nil
}

// TopicMatches tests a message type against one subscription topic.
//
// A literal topic matches only itself. A terminal-wildcard topic matches every
// message type that starts with the text before the "*", so "*" matches all of
// them. A topic with a misplaced wildcard matches nothing.
func TopicMatches(topic, messageType string) bool {
	prefix, ok := PatternPrefix(topic)
	if !ok {
		return !strings.Contains(topic, "*") && topic == messageType
	}
	return strings.HasPrefix(messageType, prefix)
}

// PartitionTopics splits requested topics into literal names and wildcard
// selectors.
//
// Order within each group is the caller's order, and duplicates are kept: the
// engine deduplicates recipients, so a connection subscribed to both
// "tick.out" and "tick.*" still receives one copy of "tick.out".
//
// It returns the first topic error found, before any subscription request is
// sent.
func PartitionTopics(topics []string) (exact []string, patterns []string, err error) {
	for _, topic := range topics {
		kind, classifyErr := ClassifyTopic(topic)
		if classifyErr != nil {
			return nil, nil, classifyErr
		}
		if kind == TopicExact {
			exact = append(exact, topic)
		} else {
			patterns = append(patterns, topic)
		}
	}
	return exact, patterns, nil
}

// IsEmergentMessageType reports whether a push notification names an Emergent
// message type.
//
// A "*" subscription matches every IPC broadcast the engine makes, which
// includes the transport's own envelope names such as "SystemEvent". Those are
// the containers Emergent messages travel in, not messages in their own right,
// and the engine forwards what they carry separately under its own type. They
// are not valid Emergent message types, which is how they are told apart.
func IsEmergentMessageType(messageType string) bool {
	return messageTypePattern.MatchString(messageType)
}
