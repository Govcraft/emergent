package emergent

import (
	"io"
	"net"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// slowUnixPair is a connected Unix socket pair whose client side has the
// smallest send buffer the kernel allows, so one large frame cannot leave in a
// single write system call.
func slowUnixPair(t *testing.T) (client *net.UnixConn, server net.Conn) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "w.sock")
	listener, err := net.Listen("unix", path)
	if err != nil {
		t.Fatalf("listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	accepted := make(chan net.Conn, 1)
	go func() {
		conn, acceptErr := listener.Accept()
		if acceptErr == nil {
			accepted <- conn
		}
	}()

	conn, err := net.Dial("unix", path)
	if err != nil {
		t.Fatalf("dial failed: %v", err)
	}
	client = conn.(*net.UnixConn)
	if err := client.SetWriteBuffer(1); err != nil {
		t.Fatalf("SetWriteBuffer failed: %v", err)
	}

	select {
	case server = <-accepted:
	case <-time.After(5 * time.Second):
		t.Fatal("accept timed out")
	}
	return client, server
}

// A frame larger than the socket buffer goes out in several write system
// calls. net.Conn.Write loops until every byte is out or returns an error, and
// holds the descriptor's write lock while it does, so whole frames arrive even
// with writeMu taken away. writeMu states the same rule where the SDK can see
// it. This test pins the outcome, not which of the two locks provides it.
func TestConcurrentPublishesStayWholeOnASlowSocket(t *testing.T) {
	const publishers = 6
	const perPublisher = 4
	const total = publishers * perPublisher

	client, _ := unitClient(t)
	conn, server := slowUnixPair(t)
	defer func() { _ = conn.Close() }()
	defer func() { _ = server.Close() }()
	client.conn = conn

	want := make(map[string]bool, total)
	var wantMu sync.Mutex
	var wg sync.WaitGroup
	for p := 0; p < publishers; p++ {
		wg.Add(1)
		go func(p int) {
			defer wg.Done()
			for i := 0; i < perPublisher; i++ {
				msg, err := NewMessage("go69.event")
				if err != nil {
					t.Errorf("NewMessage failed: %v", err)
					return
				}
				// Sizes differ so that a frame cut short cannot line up with
				// the next one by accident.
				msg.WithPayload(map[string]any{"pad": strings.Repeat("z", 200_000+1000*(p*perPublisher+i))})
				wantMu.Lock()
				want[string(msg.ID)] = true
				wantMu.Unlock()
				if err := client.publishInternal(msg); err != nil {
					t.Errorf("publishInternal failed: %v", err)
					return
				}
			}
		}(p)
	}

	// Read slowly and in small pieces so the writers block part way through.
	got := make(map[string]bool, total)
	var buffer []byte
	chunk := make([]byte, 8192)
	_ = server.SetReadDeadline(time.Now().Add(30 * time.Second))
	for len(got) < total {
		n, err := server.Read(chunk)
		if err != nil {
			if err == io.EOF {
				break
			}
			t.Fatalf("read failed after %d frames: %v", len(got), err)
		}
		buffer = append(buffer, chunk[:n]...)
		for {
			frame, decodeErr := TryDecodeFrame(buffer)
			if decodeErr != nil {
				t.Fatalf("frame %d on the wire does not decode: %v", len(got)+1, decodeErr)
			}
			if frame == nil {
				break
			}
			buffer = buffer[frame.BytesConsumed:]
			message, ok := wireMessageFromRequest(frame.Payload)
			if !ok {
				t.Fatalf("frame %d is not a publish envelope: %v", len(got)+1, frame.Payload)
			}
			got[message] = true
		}
	}
	wg.Wait()

	if len(got) != total {
		t.Fatalf("read %d whole frames, want %d", len(got), total)
	}
	for id := range want {
		if !got[id] {
			t.Errorf("message %s never arrived whole", id)
		}
	}
	if len(buffer) != 0 {
		t.Errorf("%d stray bytes left after the last frame", len(buffer))
	}
}

// wireMessageFromRequest pulls the message ID out of a publish envelope.
func wireMessageFromRequest(payload any) (string, bool) {
	envelope, ok := payload.(map[string]any)
	if !ok {
		return "", false
	}
	body, ok := envelope["payload"].(map[string]any)
	if !ok {
		return "", false
	}
	inner, ok := body["inner"].(map[string]any)
	if !ok {
		return "", false
	}
	id, ok := inner["id"].(string)
	return id, ok
}
