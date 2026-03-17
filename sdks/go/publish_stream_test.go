package emergent

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// testEngine starts a real Emergent engine for integration testing.
type testEngine struct {
	cmd        *exec.Cmd
	socketPath string
	tmpDir     string
}

// engineBinary locates the engine binary in the workspace target directory.
func engineBinary() string {
	// sdks/go -> workspace root
	goSDKDir, _ := os.Getwd()
	workspaceRoot := filepath.Dir(filepath.Dir(goSDKDir))

	debugBin := filepath.Join(workspaceRoot, "target", "debug", "emergent")
	if _, err := os.Stat(debugBin); err == nil {
		return debugBin
	}

	return filepath.Join(workspaceRoot, "target", "release", "emergent")
}

func startTestEngine(t *testing.T) *testEngine {
	t.Helper()

	binPath := engineBinary()
	if _, err := os.Stat(binPath); err != nil {
		t.Skipf("engine binary not found at %s — run 'cargo build' first", binPath)
	}

	tmpDir, err := os.MkdirTemp("", "emergent-test-*")
	if err != nil {
		t.Fatalf("failed to create temp dir: %v", err)
	}

	socketPath := filepath.Join(tmpDir, "test.sock")
	logDir := filepath.Join(tmpDir, "logs")
	dbPath := filepath.Join(tmpDir, "events.db")

	configContent := fmt.Sprintf(`
[engine]
name = "test-engine"
socket_path = "%s"
api_port = 0

[event_store]
json_log_dir = "%s"
sqlite_path = "%s"
retention_days = 1
`, socketPath, logDir, dbPath)

	configPath := filepath.Join(tmpDir, "test.toml")
	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		t.Fatalf("failed to write config: %v", err)
	}

	cmd := exec.Command(binPath, "--config", configPath)
	cmd.Stdout = nil
	cmd.Stderr = nil

	if err := cmd.Start(); err != nil {
		t.Fatalf("failed to start engine: %v", err)
	}

	// Wait for socket to appear
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(socketPath); err == nil {
			time.Sleep(100 * time.Millisecond) // let IPC listener start
			break
		}
		time.Sleep(50 * time.Millisecond)
	}

	if _, err := os.Stat(socketPath); err != nil {
		cmd.Process.Kill()
		cmd.Wait()
		os.RemoveAll(tmpDir)
		t.Fatalf("engine socket did not appear at %s", socketPath)
	}

	te := &testEngine{
		cmd:        cmd,
		socketPath: socketPath,
		tmpDir:     tmpDir,
	}

	t.Cleanup(func() {
		te.stop()
	})

	return te
}

func (te *testEngine) stop() {
	if te.cmd != nil && te.cmd.Process != nil {
		te.cmd.Process.Kill()
		te.cmd.Wait()
	}
	os.RemoveAll(te.tmpDir)
}

func TestPublishAllIntegration(t *testing.T) {
	engine := startTestEngine(t)

	// Subscribe first
	sinkOpts := &ConnectOptions{SocketPath: engine.socketPath}
	sink, err := ConnectSink("test_sink", sinkOpts)
	if err != nil {
		t.Fatalf("sink connect failed: %v", err)
	}
	defer sink.Close()

	ctx := context.Background()
	stream, err := sink.Subscribe(ctx, []string{"test.batch"})
	if err != nil {
		t.Fatalf("subscribe failed: %v", err)
	}

	// Connect source
	sourceOpts := &ConnectOptions{SocketPath: engine.socketPath}
	source, err := ConnectSource("test_source", sourceOpts)
	if err != nil {
		t.Fatalf("source connect failed: %v", err)
	}
	defer source.Close()

	// Build batch of messages
	messages := make([]*EmergentMessage, 5)
	for i := range 5 {
		msg, _ := NewMessage("test.batch")
		msg.WithPayload(map[string]any{"index": i})
		messages[i] = msg
	}

	// PublishAll
	count, err := source.PublishAll(messages)
	if err != nil {
		t.Fatalf("PublishAll failed: %v", err)
	}
	if count != 5 {
		t.Errorf("expected 5 published, got %d", count)
	}

	// Collect messages with timeout
	received := make([]*EmergentMessage, 0, 5)
	deadline := time.After(5 * time.Second)
	for len(received) < 5 {
		select {
		case msg := <-stream.C():
			received = append(received, msg)
		case <-deadline:
			t.Fatalf("timed out waiting for messages, got %d of 5", len(received))
		}
	}

	if len(received) != 5 {
		t.Errorf("expected 5 messages, got %d", len(received))
	}
}

func TestPublishStreamIntegration(t *testing.T) {
	engine := startTestEngine(t)

	// Subscribe first
	sinkOpts := &ConnectOptions{SocketPath: engine.socketPath}
	sink, err := ConnectSink("test_sink", sinkOpts)
	if err != nil {
		t.Fatalf("sink connect failed: %v", err)
	}
	defer sink.Close()

	ctx := context.Background()
	stream, err := sink.Subscribe(ctx, []string{"test.stream"})
	if err != nil {
		t.Fatalf("subscribe failed: %v", err)
	}

	// Connect source
	sourceOpts := &ConnectOptions{SocketPath: engine.socketPath}
	source, err := ConnectSource("test_source", sourceOpts)
	if err != nil {
		t.Fatalf("source connect failed: %v", err)
	}
	defer source.Close()

	// Create channel-based stream
	ch := make(chan *EmergentMessage, 16)
	go func() {
		defer close(ch)
		for i := range 3 {
			msg, _ := NewMessage("test.stream")
			msg.WithPayload(map[string]any{"seq": i})
			ch <- msg
		}
	}()

	// PublishStream
	count, err := source.PublishStream(ctx, ch)
	if err != nil {
		t.Fatalf("PublishStream failed: %v", err)
	}
	if count != 3 {
		t.Errorf("expected 3 published, got %d", count)
	}

	// Collect messages
	received := make([]*EmergentMessage, 0, 3)
	deadline := time.After(5 * time.Second)
	for len(received) < 3 {
		select {
		case msg := <-stream.C():
			received = append(received, msg)
		case <-deadline:
			t.Fatalf("timed out waiting for messages, got %d of 3", len(received))
		}
	}

	if len(received) != 3 {
		t.Errorf("expected 3 messages, got %d", len(received))
	}
}

func TestPublishAllEmptySlice(t *testing.T) {
	engine := startTestEngine(t)

	sourceOpts := &ConnectOptions{SocketPath: engine.socketPath}
	source, err := ConnectSource("test_source", sourceOpts)
	if err != nil {
		t.Fatalf("source connect failed: %v", err)
	}
	defer source.Close()

	count, err := source.PublishAll([]*EmergentMessage{})
	if err != nil {
		t.Fatalf("PublishAll failed: %v", err)
	}
	if count != 0 {
		t.Errorf("expected 0 published, got %d", count)
	}
}
