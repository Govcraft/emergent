// Console Sink (Go) - subscribes to events and prints them to stdout.
//
// Usage:
//
//	go build -o console-go . && ./console-go
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"time"

	emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
	err := emergent.RunSink("", []string{
		"timer.tick",
		"timer.filtered",
		"filter.processed",
		"system.started.*",
		"system.stopped.*",
		"system.error.*",
	}, func(msg *emergent.EmergentMessage) error {
		timestamp := time.Now().Format("15:04:05.000")
		formatted := formatMessage(msg)
		fmt.Printf("[%s] %s\n", timestamp, formatted)
		return nil
	})

	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}

func formatMessage(msg *emergent.EmergentMessage) string {
	mt := string(msg.MessageType)

	switch {
	case startsWith(mt, "system.started."):
		return formatSystemEvent("STARTED", msg)
	case startsWith(mt, "system.stopped."):
		return formatSystemEvent("STOPPED", msg)
	case startsWith(mt, "system.error."):
		return formatSystemEvent("ERROR", msg)
	case mt == "timer.tick":
		return formatTimerTick(msg)
	case mt == "timer.filtered":
		return formatTimerFiltered(msg)
	case mt == "filter.processed":
		return formatFilterProcessed(msg)
	default:
		payload, _ := json.Marshal(msg.Payload)
		return fmt.Sprintf("[%s] %s", mt, string(payload))
	}
}

func formatSystemEvent(label string, msg *emergent.EmergentMessage) string {
	var p emergent.SystemEventPayload
	if err := msg.PayloadAs(&p); err != nil {
		return fmt.Sprintf("[%s] %v", label, msg.Payload)
	}
	id := string(msg.ID)
	if len(id) > 8 {
		id = id[:8]
	}
	if p.IsError() && p.Error != nil {
		return fmt.Sprintf("[%s] %s (%s): %s", label, p.Name, p.Kind, *p.Error)
	}
	return fmt.Sprintf("[%s] %s (%s) [%s]", label, p.Name, p.Kind, id)
}

func formatTimerTick(msg *emergent.EmergentMessage) string {
	var tick struct {
		Sequence   int64 `json:"sequence"`
		IntervalMs int64 `json:"interval_ms"`
	}
	if err := msg.PayloadAs(&tick); err != nil {
		return fmt.Sprintf("[timer.tick] %v", msg.Payload)
	}
	return fmt.Sprintf("[TICK] #%d (every %dms)", tick.Sequence, tick.IntervalMs)
}

func formatTimerFiltered(msg *emergent.EmergentMessage) string {
	var p struct {
		OriginalSequence int64  `json:"original_sequence"`
		FilterReason     string `json:"filter_reason"`
		FilterEvery      int64  `json:"filter_every"`
	}
	if err := msg.PayloadAs(&p); err != nil {
		return fmt.Sprintf("[timer.filtered] %v", msg.Payload)
	}
	return fmt.Sprintf("[FILTERED] tick #%d (%s every %d)", p.OriginalSequence, p.FilterReason, p.FilterEvery)
}

func formatFilterProcessed(msg *emergent.EmergentMessage) string {
	var p struct {
		OriginalSequence int64  `json:"original_sequence"`
		Action           string `json:"action"`
		Reason           string `json:"reason"`
	}
	if err := msg.PayloadAs(&p); err != nil {
		return fmt.Sprintf("[filter.processed] %v", msg.Payload)
	}
	return fmt.Sprintf("[FILTER] tick #%d %s %s", p.OriginalSequence, p.Action, p.Reason)
}

func startsWith(s, prefix string) bool {
	return len(s) >= len(prefix) && s[:len(prefix)] == prefix
}
