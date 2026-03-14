package emergent

import (
	"testing"
)

func TestResolveName_Explicit(t *testing.T) {
	name := resolveName("my_source", "default")
	if name != "my_source" {
		t.Errorf("expected my_source, got: %s", name)
	}
}

func TestResolveName_EnvVar(t *testing.T) {
	t.Setenv(emergentNameEnv, "env_source")

	name := resolveName("", "default")
	if name != "env_source" {
		t.Errorf("expected env_source, got: %s", name)
	}
}

func TestResolveName_Default(t *testing.T) {
	t.Setenv(emergentNameEnv, "")

	name := resolveName("", "source")
	if name != "source" {
		t.Errorf("expected source, got: %s", name)
	}
}

func TestResolveName_ExplicitOverridesEnv(t *testing.T) {
	t.Setenv(emergentNameEnv, "env_source")

	name := resolveName("explicit_source", "default")
	if name != "explicit_source" {
		t.Errorf("expected explicit_source, got: %s", name)
	}
}
