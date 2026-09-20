package emergent

import (
	"context"
	"net"
	"path/filepath"
	"testing"
	"time"
)

// engineListener listens on a Unix socket standing in for the engine's and
// points EMERGENT_SOCKET at it.
func engineListener(t *testing.T) *net.UnixListener {
	t.Helper()
	t.Setenv("EMERGENT_LOG", "off")
	path := filepath.Join(t.TempDir(), "e.sock")
	listener, err := net.ListenUnix("unix", &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		t.Fatalf("listen failed: %v", err)
	}
	t.Cleanup(func() { listener.Close() })
	t.Setenv("EMERGENT_SOCKET", path)
	return listener
}

// A Source subscribes to nothing, so no stream ends to tell it the engine is
// gone. The context is the only thing its function waits on, so a lost
// connection has to cancel it, the way SIGTERM does.
func TestRunSourceCancelsItsContextWhenTheEngineLeaves(t *testing.T) {
	endings := []struct {
		name string
		end  func(t *testing.T, peer net.Conn)
	}{
		{"peer reads everything then closes", func(t *testing.T, peer net.Conn) {
			// One publish was written. Take it, so the close is a clean EOF.
			if _, err := peer.Read(make([]byte, readBufferSize)); err != nil {
				t.Errorf("peer read failed: %v", err)
			}
			peer.Close()
		}},
		{"peer closes with bytes unread", func(_ *testing.T, peer net.Conn) {
			peer.Close()
		}},
	}

	for _, ending := range endings {
		t.Run(ending.name, func(t *testing.T) {
			listener := engineListener(t)
			published := make(chan struct{})
			giveUp := make(chan struct{})
			cancelled := make(chan bool, 1)
			finished := make(chan error, 1)

			go func() {
				finished <- RunSource("go83", func(ctx context.Context, source *EmergentSource) error {
					msg, err := NewMessage("go83.event")
					if err != nil {
						return err
					}
					if err := source.Publish(msg); err != nil {
						return err
					}
					close(published)
					select {
					case <-ctx.Done():
						cancelled <- true
					case <-giveUp:
						cancelled <- false
					}
					return nil
				})
			}()

			peer, err := listener.Accept()
			if err != nil {
				t.Fatalf("accept failed: %v", err)
			}
			<-published

			ending.end(t, peer)

			select {
			case err := <-finished:
				if err != nil {
					t.Errorf("RunSource = %v, want nil", err)
				}
			case <-time.After(2 * time.Second):
				// Let the source function return, so the failing run leaves
				// nothing running.
				close(giveUp)
				<-finished
			}
			if !<-cancelled {
				t.Error("the context was still live 2s after the engine left")
			}
		})
	}
}

// A function that returns on its own is handed a live context.
func TestRunSourceLeavesTheContextLiveWhileTheEngineStays(t *testing.T) {
	listener := engineListener(t)
	finished := make(chan error, 1)
	live := make(chan bool, 1)

	go func() {
		finished <- RunSource("go83", func(ctx context.Context, _ *EmergentSource) error {
			live <- ctx.Err() == nil
			return nil
		})
	}()

	peer, err := listener.Accept()
	if err != nil {
		t.Fatalf("accept failed: %v", err)
	}
	defer peer.Close()

	if err := <-finished; err != nil {
		t.Errorf("RunSource = %v, want nil", err)
	}
	if !<-live {
		t.Error("the context was cancelled with the engine still there")
	}
}

// Close is the caller's own doing, not a lost connection.
func TestCloseDoesNotReportALostConnection(t *testing.T) {
	listener := engineListener(t)
	source, err := ConnectSource("go83", nil)
	if err != nil {
		t.Fatalf("connect failed: %v", err)
	}
	peer, err := listener.Accept()
	if err != nil {
		t.Fatalf("accept failed: %v", err)
	}
	defer peer.Close()

	if err := source.Close(); err != nil {
		t.Fatalf("close failed: %v", err)
	}

	select {
	case <-source.lost:
		t.Error("lost was closed by Close")
	default:
	}
}
