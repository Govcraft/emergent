// Filter Handler (Go) - subscribes to timer.tick events and publishes
// timer.filtered for every Nth tick.
//
// Usage:
//
//	go build -o filter-go . && ./filter-go
//	go build -o filter-go . && ./filter-go --filter-every 10
package main

import (
	"flag"
	"fmt"
	"os"

	emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
	filterEvery := flag.Int64("filter-every", 5, "Pass through every Nth tick")
	flag.Parse()

	err := emergent.RunHandler("", []string{"timer.tick"}, func(msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
		// Parse the timer tick payload
		var tick struct {
			Sequence   int64 `json:"sequence"`
			IntervalMs int64 `json:"interval_ms"`
		}
		if err := msg.PayloadAs(&tick); err != nil {
			return nil // Skip malformed payloads
		}

		passed := tick.Sequence%*filterEvery == 0

		if passed {
			// Publish timer.filtered for ticks that pass
			out, err := emergent.NewMessage("timer.filtered")
			if err != nil {
				return err
			}
			out.WithCausationFromMessage(msg.ID).
				WithPayload(map[string]any{
					"original_sequence":    tick.Sequence,
					"original_interval_ms": tick.IntervalMs,
					"filter_reason":        fmt.Sprintf("every_%dth", *filterEvery),
					"filter_every":         *filterEvery,
				})
			if err := handler.Publish(out); err != nil {
				return err
			}
		}

		// Always publish filter.processed status
		status, err := emergent.NewMessage("filter.processed")
		if err != nil {
			return err
		}
		action := "filtered"
		reason := fmt.Sprintf("not_multiple_of_%d", *filterEvery)
		if passed {
			action = "passed"
			reason = fmt.Sprintf("every_%dth", *filterEvery)
		}
		status.WithCausationFromMessage(msg.ID).
			WithPayload(map[string]any{
				"original_sequence": tick.Sequence,
				"action":            action,
				"reason":            reason,
			})
		return handler.Publish(status)
	})

	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}
