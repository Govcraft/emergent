// Package emergent provides the Go SDK for the Emergent workflow engine.
//
// Emergent is an event-driven workflow engine that implements a publish-subscribe
// pattern using three primitive types communicating via Unix IPC sockets:
//
//   - [EmergentSource]: publish-only (ingress)
//   - [EmergentHandler]: subscribe and publish (transformation)
//   - [EmergentSink]: subscribe-only (egress)
//
// # Quick Start
//
// Use the helper functions for the simplest experience:
//
//	// Source: emit events
//	emergent.RunSource("my_timer", func(ctx context.Context, source *emergent.EmergentSource) error {
//	    ticker := time.NewTicker(3 * time.Second)
//	    defer ticker.Stop()
//	    for {
//	        select {
//	        case <-ctx.Done():
//	            return nil
//	        case <-ticker.C:
//	            msg, _ := emergent.NewMessage("timer.tick")
//	            msg.WithPayload(map[string]any{"count": 1})
//	            source.Publish(msg)
//	        }
//	    }
//	})
//
//	// Handler: transform events
//	emergent.RunHandler("my_filter", []string{"timer.tick"}, func(msg *emergent.EmergentMessage, h *emergent.EmergentHandler) error {
//	    out, _ := emergent.NewMessage("timer.filtered")
//	    out.WithCausationFromMessage(msg.ID).WithPayload(msg.Payload)
//	    return h.Publish(out)
//	})
//
//	// Sink: consume events
//	emergent.RunSink("my_console", []string{"timer.filtered"}, func(msg *emergent.EmergentMessage) error {
//	    fmt.Println(msg.MessageType, msg.Payload)
//	    return nil
//	})
package emergent

// Version is the SDK version, matching the Emergent release cycle.
const Version = "0.10.5"
