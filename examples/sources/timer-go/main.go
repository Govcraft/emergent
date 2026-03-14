// Timer Source (Go) - emits timer.tick events at a configurable interval.
//
// Usage:
//
//	go build -o timer-go . && ./timer-go
//	go build -o timer-go . && ./timer-go --interval 1000
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"time"

	emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
	interval := flag.Int64("interval", 5000, "Interval in milliseconds between ticks")
	maxTicks := flag.Int64("max-ticks", 0, "Maximum number of ticks to emit (0 = unlimited)")
	flag.Parse()

	err := emergent.RunSource("", func(ctx context.Context, source *emergent.EmergentSource) error {
		ticker := time.NewTicker(time.Duration(*interval) * time.Millisecond)
		defer ticker.Stop()

		var sequence int64

		for {
			select {
			case <-ctx.Done():
				return nil
			case <-ticker.C:
				sequence++

				msg, err := emergent.NewMessage("timer.tick")
				if err != nil {
					return err
				}
				msg.WithPayload(map[string]any{
					"sequence":    sequence,
					"interval_ms": *interval,
				})

				if err := source.Publish(msg); err != nil {
					return err
				}

				if *maxTicks > 0 && sequence >= *maxTicks {
					return nil
				}
			}
		}
	})

	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}
