package emergent

import (
	"os"
	"regexp"
	"testing"
)

// Version said 0.10.5 through SDK 0.13.1 because nothing tied it to a release.
// A Go module has no manifest version, so the workspace version that all four
// SDKs are released under is the reference.
func TestVersionIsTheWorkspaceVersion(t *testing.T) {
	manifest, err := os.ReadFile("../../Cargo.toml")
	if err != nil {
		t.Skipf("no workspace manifest beside the module: %v", err)
	}
	found := regexp.MustCompile(`(?m)^version = "([^"]+)"`).FindSubmatch(manifest)
	if found == nil {
		t.Fatal("the workspace manifest declares no version")
	}
	if released := string(found[1]); Version != released {
		t.Errorf("Version = %q, the workspace releases %q", Version, released)
	}
}
