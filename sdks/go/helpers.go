package emergent

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"
)

// SourceRunFunc is the callback signature for RunSource.
// The context is cancelled when SIGTERM/SIGINT is received.
type SourceRunFunc func(ctx context.Context, source *EmergentSource) error

// HandlerProcessFunc is the callback signature for RunHandler.
// Called for each incoming message with the handler for publishing responses.
type HandlerProcessFunc func(msg *EmergentMessage, handler *EmergentHandler) error

// SinkConsumeFunc is the callback signature for RunSink.
// Called for each incoming message.
type SinkConsumeFunc func(msg *EmergentMessage) error

// RunSource connects as a Source, sets up signal handling, and runs the user function.
//
// The function receives a context that is cancelled on SIGTERM/SIGINT.
// On return, the source is automatically disconnected.
//
// Example:
//
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
func RunSource(name string, fn SourceRunFunc) error {
	resolvedName := resolveName(name, "source")

	source, err := ConnectSource(resolvedName, nil)
	if err != nil {
		return &HelperError{Msg: fmt.Sprintf("failed to connect as '%s': %v", resolvedName, err)}
	}
	defer source.Close()

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()

	if err := fn(ctx, source); err != nil {
		return &HelperError{Msg: fmt.Sprintf("user function error: %v", err)}
	}
	return nil
}

// RunHandler connects as a Handler, subscribes to the given topics,
// and calls the process function for each message.
//
// Exits on SIGTERM/SIGINT or when the message stream closes (engine shutdown).
//
// Example:
//
//	emergent.RunHandler("my_filter", []string{"timer.tick"}, func(msg *emergent.EmergentMessage, h *emergent.EmergentHandler) error {
//	    out, _ := emergent.NewMessage("timer.filtered")
//	    out.WithCausationFromMessage(msg.ID).WithPayload(msg.Payload)
//	    return h.Publish(out)
//	})
func RunHandler(name string, subscriptions []string, fn HandlerProcessFunc) error {
	resolvedName := resolveName(name, "handler")

	handler, err := ConnectHandler(resolvedName, nil)
	if err != nil {
		return &HelperError{Msg: fmt.Sprintf("failed to connect as '%s': %v", resolvedName, err)}
	}
	defer handler.Close()

	stream, err := handler.Subscribe(context.Background(), subscriptions)
	if err != nil {
		return &HelperError{Msg: fmt.Sprintf("failed to subscribe: %v", err)}
	}
	defer stream.Close()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
	defer signal.Stop(sigCh)

	for {
		select {
		case <-sigCh:
			return nil
		case msg, ok := <-stream.C():
			if !ok {
				return nil // Stream closed (engine shutdown)
			}
			if err := fn(msg, handler); err != nil {
				return &HelperError{Msg: fmt.Sprintf("user function error: %v", err)}
			}
		}
	}
}

// RunSink connects as a Sink, subscribes to the given topics,
// and calls the consume function for each message.
//
// Exits on SIGTERM/SIGINT or when the message stream closes (engine shutdown).
//
// Example:
//
//	emergent.RunSink("my_console", []string{"timer.filtered"}, func(msg *emergent.EmergentMessage) error {
//	    fmt.Println(msg.MessageType, msg.Payload)
//	    return nil
//	})
func RunSink(name string, subscriptions []string, fn SinkConsumeFunc) error {
	resolvedName := resolveName(name, "sink")

	sink, err := ConnectSink(resolvedName, nil)
	if err != nil {
		return &HelperError{Msg: fmt.Sprintf("failed to connect as '%s': %v", resolvedName, err)}
	}
	defer sink.Close()

	stream, err := sink.Subscribe(context.Background(), subscriptions)
	if err != nil {
		return &HelperError{Msg: fmt.Sprintf("failed to subscribe: %v", err)}
	}
	defer stream.Close()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
	defer signal.Stop(sigCh)

	for {
		select {
		case <-sigCh:
			return nil
		case msg, ok := <-stream.C():
			if !ok {
				return nil // Stream closed (engine shutdown)
			}
			if err := fn(msg); err != nil {
				return &HelperError{Msg: fmt.Sprintf("user function error: %v", err)}
			}
		}
	}
}
