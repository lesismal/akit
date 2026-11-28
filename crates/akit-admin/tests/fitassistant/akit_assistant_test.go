// The assistant's store section between fit and akit (akit's I160): copied into fit's package
// assistant at the compatibility gate's pin and run there with
//
//	AKIT_ASSISTANT_DIR=<dir> go test -run TestAkitAssistant ./assistant
//
// after akit's `fit_interop` test wrote <dir>/akit.section, the section akit's Settings page
// saved. fit opens it as its own, then saves settings of its own with a key and writes the
// section it kept to <dir>/fit.section, which akit's `fit_interop` reads in turn.
package assistant

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAkitAssistant(t *testing.T) {
	dir := os.Getenv("AKIT_ASSISTANT_DIR")
	if dir == "" {
		t.Skip("AKIT_ASSISTANT_DIR is not set")
	}
	data, err := os.ReadFile(filepath.Join(dir, "akit.section"))
	if err != nil {
		t.Fatal(err)
	}
	st := &memStore{sec: map[string][]byte{Section: data}}
	f := start(t, loadConfig(t, `{"providers": [{"name": "o", "kind": "ollama"}]}`), st)
	s := f.settings()
	if s.Source != "store" || !strings.Contains(string(s.Settings), `"default_level":"off"`) {
		t.Fatalf("akit's settings: %s %s", s.Source, s.Settings)
	}
	if k := s.Keys["claude"]; !k.Set || k.Source != "store" || k.Masked != "••••cdef" {
		t.Fatalf("akit's key: %+v", k)
	}
	status, body := f.do("PUT", "/api/m/ai/settings", `{"settings": {"providers": [{"name": "o", "kind": "ollama", "models": [{"name": "llama3"}]}], "users": [{"username": "bob", "level": "propose"}]}, "keys": {"o": "`+testKey+`"}, "version": "`+s.Version+`"}`)
	if status != 200 {
		t.Fatalf("PUT: %d %s", status, body)
	}
	if err := os.WriteFile(filepath.Join(dir, "fit.section"), st.Section(Section), 0o600); err != nil {
		t.Fatal(err)
	}
}
