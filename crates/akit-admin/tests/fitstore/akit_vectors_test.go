// akit's interop vectors for fit's store package (akit I93), run inside fit's own
// admin/store package so that fit's own code writes and reads them. Copy this file
// into fit's admin/store and run:
//
//	AKIT_VECTORS_OUT=<dir> go test -run TestAkitVectors ./admin/store
//	AKIT_WROTE_DIR=<dir> go test -run TestAkitWrote ./admin/store
//
// TestAkitVectors writes stores of every version, one written back and with a
// password changed as fit run and its console do, and an export, with
// vectors.json saying what each holds; TestAkitWrote opens the stores akit wrote
// (akit's own, and fit's written back by akit), as its manifest.json says, and
// checks what they hold.
package store

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

type vector struct {
	File     string                     `json:"file"`
	Password string                     `json:"password"`
	Export   bool                       `json:"export,omitempty"`
	Config   string                     `json:"config"`
	Users    []User                     `json:"users,omitempty"`
	Sections map[string]json.RawMessage `json:"sections,omitempty"`
}

var cheap = Params{Time: 1, MemoryKiB: 64, Threads: 1}

func TestAkitVectors(t *testing.T) {
	out := os.Getenv("AKIT_VECTORS_OUT")
	if out == "" {
		t.Skip("AKIT_VECTORS_OUT not set")
	}
	saved := DefaultParams
	defer func() { DefaultParams = saved }()
	config := `{"listeners": [{"name": "web", "address": ":8080"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "hé é <&>"}}}]}`
	users := []User{{Username: "ada", PasswordHash: "$2a$04$ada"}, {Username: "bob", PasswordHash: "$2a$04$bob\"q"}}
	var vs []vector
	write := func(name, pw string, p Params, f func(s *Store) error) {
		DefaultParams = p
		path := filepath.Join(out, name)
		_ = os.Remove(path)
		s, err := Create(path, pw)
		if err != nil {
			t.Fatal(err)
		}
		if err := f(s); err != nil {
			t.Fatal(err)
		}
		v := vector{File: name, Password: pw, Config: string(s.data), Users: s.users, Sections: s.sections}
		vs = append(vs, v)
	}
	// Version 1: the config alone.
	write("v1.store", "fit-v1", cheap, func(s *Store) error {
		s.data = []byte(config)
		return os.WriteFile(s.path, s.seal(versionConfig, []byte(config)), 0o600)
	})
	write("v2.store", "fit-v2", cheap, func(s *Store) error {
		if err := s.Write([]byte(config)); err != nil {
			return err
		}
		return s.SetUsers(users)
	})
	write("v3.store", "fit-v3", cheap, func(s *Store) error {
		if err := s.Write([]byte(config)); err != nil {
			return err
		}
		if err := s.SetUsers(users); err != nil {
			return err
		}
		if err := s.SetSection("assistant", []byte(`{"model": "m", "keys": ["k1", "k2"]}`)); err != nil {
			return err
		}
		return s.SetSection("cluster", []byte(`{"version":2,"members":[]}`))
	})
	// What fit run and its console do to the store they keep the config in (akit I93b): the
	// store opened again at a restart, the config written back after a change (Write, the
	// users and sections kept), and a console password changed (SetUsers, the config kept).
	write("writeback.store", "fit-writeback", cheap, func(s *Store) error {
		if err := s.Write([]byte(config)); err != nil {
			return err
		}
		if err := s.SetUsers(users); err != nil {
			return err
		}
		if err := s.SetSection("cluster", []byte(`{"version":2,"members":[]}`)); err != nil {
			return err
		}
		again, data, err := Open(s.path, "fit-writeback")
		if err != nil {
			return err
		}
		if string(data) != config {
			t.Fatalf("writeback.store: opened again: %q", data)
		}
		if err := again.Write([]byte(`{"routes": [{"handler": {"module": "respond", "params": {"body": "written back"}}}]}`)); err != nil {
			return err
		}
		if err := again.SetUsers([]User{{Username: "ada", PasswordHash: "$2a$10$ada-changed"}, users[1]}); err != nil {
			return err
		}
		_, err = s.Read()
		return err
	})
	// fit's own settings, as a store fit makes has them.
	write("v2-default.store", "fit-default", saved, func(s *Store) error {
		if err := s.Write([]byte(`{"routes":[]}`)); err != nil {
			return err
		}
		return s.SetUsers(users[:1])
	})
	DefaultParams = cheap
	file, err := Seal("export-pw", []byte(config), users)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(out, "export.store"), file, 0o600); err != nil {
		t.Fatal(err)
	}
	vs = append(vs, vector{File: "export.store", Password: "export-pw", Export: true, Config: config, Users: users})
	b, _ := json.MarshalIndent(vs, "", "  ")
	if err := os.WriteFile(filepath.Join(out, "vectors.json"), append(b, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestAkitWrote(t *testing.T) {
	dir := os.Getenv("AKIT_WROTE_DIR")
	if dir == "" {
		t.Skip("AKIT_WROTE_DIR not set")
	}
	b, err := os.ReadFile(filepath.Join(dir, "manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var vs []vector
	if err := json.Unmarshal(b, &vs); err != nil {
		t.Fatal(err)
	}
	if len(vs) == 0 {
		t.Fatal("no stores")
	}
	for _, v := range vs {
		path := filepath.Join(dir, v.File)
		var data []byte
		var users []User
		var sections map[string]json.RawMessage
		if v.Export {
			file, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if data, users, err = Unseal(file, v.Password); err != nil {
				t.Fatalf("%s: %v", v.File, err)
			}
		} else {
			s, d, err := Open(path, v.Password)
			if err != nil {
				t.Fatalf("%s: %v", v.File, err)
			}
			data, users, sections = d, s.Users(), s.sections
		}
		if string(data) != v.Config {
			t.Errorf("%s: config %q, want %q", v.File, data, v.Config)
		}
		if !slices.Equal(users, v.Users) && !(len(users) == 0 && len(v.Users) == 0) {
			t.Errorf("%s: users %v, want %v", v.File, users, v.Users)
		}
		if len(sections) != len(v.Sections) {
			t.Errorf("%s: sections %v, want %v", v.File, sections, v.Sections)
		}
		for k, want := range v.Sections {
			var a, b any
			_ = json.Unmarshal(sections[k], &a)
			_ = json.Unmarshal(want, &b)
			ab, _ := json.Marshal(a)
			bb, _ := json.Marshal(b)
			if string(ab) != string(bb) {
				t.Errorf("%s: section %s %s, want %s", v.File, k, sections[k], want)
			}
		}
		t.Logf("%s: opened by fit", v.File)
	}
}
