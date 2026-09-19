package emergent

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestAwaitPubSubResult(t *testing.T) {
	t.Run("returns the result and keeps the pending entry alone", func(t *testing.T) {
		resultCh := make(chan any, 1)
		resultCh <- []string{"timer.tick"}
		forgotten := false

		result, err := awaitPubSubResult(context.Background(), resultCh, time.NewTimer(time.Hour), func() { forgotten = true })
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if subs, ok := result.([]string); !ok || len(subs) != 1 {
			t.Fatalf("unexpected result: %v", result)
		}
		if forgotten {
			t.Error("forget ran although the lookup was answered")
		}
	})

	t.Run("a cancelled context wins, stops the timer and forgets the request", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		timer := time.NewTimer(time.Hour)
		forgotten := false

		result, err := awaitPubSubResult(ctx, make(chan any), timer, func() { forgotten = true })
		if !errors.Is(err, context.Canceled) || result != nil {
			t.Fatalf("awaitPubSubResult() = (%v, %v), want context.Canceled", result, err)
		}
		if !forgotten {
			t.Error("forget did not run")
		}
		if timer.Stop() {
			t.Error("the request timer was left running")
		}
	})
}

// TestLookupsHonorContext checks that GetTopology and GetMySubscriptions stop
// waiting when their context ends. The fake engine acknowledges the subscribe
// and then never answers, and the client timeout is far beyond the test's
// deadline, so only the context can end the call.
func TestLookupsHonorContext(t *testing.T) {
	t.Setenv("EMERGENT_LOG", "off")

	lookups := []struct {
		name string
		call func(context.Context, *EmergentSink) error
	}{
		{"GetTopology", func(ctx context.Context, sink *EmergentSink) error {
			_, err := sink.GetTopology(ctx)
			return err
		}},
		{"GetMySubscriptions", func(ctx context.Context, sink *EmergentSink) error {
			_, err := sink.GetMySubscriptions(ctx)
			return err
		}},
	}

	contexts := []struct {
		name    string
		make    func() (context.Context, context.CancelFunc)
		wantErr error
	}{
		{"cancelled while waiting", func() (context.Context, context.CancelFunc) {
			ctx, cancel := context.WithCancel(context.Background())
			time.AfterFunc(50*time.Millisecond, cancel)
			return ctx, cancel
		}, context.Canceled},
		{"deadline while waiting", func() (context.Context, context.CancelFunc) {
			return context.WithTimeout(context.Background(), 50*time.Millisecond)
		}, context.DeadlineExceeded},
		{"cancelled before the call", func() (context.Context, context.CancelFunc) {
			ctx, cancel := context.WithCancel(context.Background())
			cancel()
			return ctx, cancel
		}, context.Canceled},
	}

	for _, lookup := range lookups {
		for _, tc := range contexts {
			t.Run(lookup.name+"/"+tc.name, func(t *testing.T) {
				engine := startFakeEngine(t, 0)
				engine.muteLookups.Store(true)
				sink := connectTestSink(t, engine, time.Minute)

				ctx, cancel := tc.make()
				defer cancel()

				done := make(chan error, 1)
				go func() { done <- lookup.call(ctx, sink) }()

				select {
				case err := <-done:
					if !errors.Is(err, tc.wantErr) {
						t.Fatalf("got %v, want %v", err, tc.wantErr)
					}
				case <-time.After(streamDeadline):
					t.Fatal("the lookup ignored its context")
				}

				assertNoPendingRequests(t, sink.baseClient)
			})
		}
	}
}
