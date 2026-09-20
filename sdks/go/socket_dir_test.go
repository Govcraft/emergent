package emergent

import (
	"os"
	"testing"
)

// socketDir is a temporary directory for a test's Unix socket. A socket path
// is limited to about 108 bytes and t.TempDir() embeds the test and subtest
// names, which alone can pass that on a Go release that does not shorten them,
// so the directory is named by a short pattern instead.
func socketDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "emg-*")
	if err != nil {
		t.Fatalf("failed to create a socket directory: %v", err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}
