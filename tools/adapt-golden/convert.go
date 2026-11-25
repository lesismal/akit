// The golden files of akit's nginx and Caddyfile converters (I74), written by fit's own
// adapter/nginx and adapter/caddyfile:
//
//   - crates/akit-config/testdata/convert/nginx.txt and caddyfile.txt: each converter on a
//     corpus: fit's sample configs (adapter/*/testdata, the assistant's migration cases, the
//     benchmark's configs, the docs' Caddyfiles), its fuzz seeds, and every string literal in
//     the converters' tests (for nginx also wrapped in a server block, as its tests' server
//     helper does), and each sample and seed cut short at eleven places. A line: the input, then "OK" and the JSON and the warnings joined with line
//     breaks, or "ERR" and the error, tab-separated, each hex-encoded; after an "OK", whether fit
//     loads the JSON: "" or the error config.Load gives, hex-encoded. The Caddyfiles are read
//     with the environment holding only FIT_TEST_PORT=9000 (akit's tests read them alike).
//   - crates/akit/tests/testdata/adapt/convert-cases.txt and convert/: `fit adapt -from nginx`
//     and `-from caddyfile` on the sample configs (copied to crates/akit/tests/testdata/adapt/
//     convert), to each target: input, from, to, exit code, stdout golden, stderr golden.
package main

import (
	"bytes"
	"encoding/hex"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/lesismal/fit/adapter/caddyfile"
	"github.com/lesismal/fit/adapter/nginx"
	"github.com/lesismal/fit/config"
)

const (
	convertDir   = "crates/akit-config/testdata/convert"
	convertCases = cliDir + "/convert"
)

// convertGolden writes the converters' golden files; fit is fit's checkout.
func convertGolden(fit string) {
	os.Clearenv()
	must(os.Setenv("FIT_TEST_PORT", "9000"))
	must(os.MkdirAll(convertDir, 0o755))
	type conv struct {
		name    string
		convert func([]byte) ([]byte, []string, error)
		files   []string
		tests   string
		wrap    func(string) string
	}
	nginxConv := func(b []byte) ([]byte, []string, error) {
		r, err := nginx.Convert(b)
		if err != nil {
			return nil, nil, err
		}
		return r.JSON, r.Warnings, nil
	}
	caddyConv := func(b []byte) ([]byte, []string, error) {
		r, err := caddyfile.Convert(b)
		if err != nil {
			return nil, nil, err
		}
		return r.JSON, r.Warnings, nil
	}
	glob := func(pats ...string) []string {
		var out []string
		for _, p := range pats {
			m, _ := filepath.Glob(filepath.Join(fit, p))
			sort.Strings(m)
			out = append(out, m...)
		}
		return out
	}
	convs := []conv{
		{"nginx", nginxConv, glob("adapter/nginx/testdata/*.conf", "assistant/testdata/eval/migrate/nginx-*/source.conf",
			"bench/configs/nginx/*.conf", "bench/vod/nginx.conf", "bench/results/*/nginx*.conf", "bench/results/*/*/nginx*.conf"),
			"adapter/nginx", func(body string) string { return "http {\n server {\n" + body + "\n }\n}\n" }},
		{"caddyfile", caddyConv, glob("adapter/caddyfile/testdata/*.caddyfile", "assistant/testdata/eval/migrate/caddy-*/source.caddyfile",
			"bench/configs/caddy/Caddyfile"),
			"adapter/caddyfile", nil},
	}
	for _, c := range convs {
		var inputs []string
		seen := map[string]bool{}
		add := func(s string) {
			if !seen[s] {
				seen[s] = true
				inputs = append(inputs, s)
			}
		}
		for _, f := range c.files {
			b, err := os.ReadFile(f)
			must(err)
			add(string(b))
		}
		if c.name == "caddyfile" {
			for _, s := range docBlocks(fit, "caddyfile") {
				add(s)
			}
		}
		for _, s := range fuzzSeeds(filepath.Join(fit, c.tests, "testdata/fuzz/FuzzConvert")) {
			add(s)
		}
		// Each sample and seed cut short at a dozen places, as configs broken midway.
		for _, s := range slices.Clone(inputs) {
			for i := 1; i < 12; i++ {
				n := len(s) * i / 12
				for n > 0 && !utf8.RuneStart(s[n]) {
					n--
				}
				add(s[:n])
			}
		}
		for _, s := range testLiterals(filepath.Join(fit, c.tests)) {
			add(s)
			if c.wrap != nil {
				add(c.wrap(s))
			}
		}
		var w strings.Builder
		for _, in := range inputs {
			out, warns, err := c.convert([]byte(in))
			if err != nil {
				fmt.Fprintf(&w, "%s\tERR\t%s\t\n", hex.EncodeToString([]byte(in)), hex.EncodeToString([]byte(err.Error())))
				continue
			}
			load := ""
			if _, err := config.Load(out); err != nil {
				load = err.Error()
			}
			fmt.Fprintf(&w, "%s\tOK\t%s\t%s\t%s\n", hex.EncodeToString([]byte(in)), hex.EncodeToString(out),
				hex.EncodeToString([]byte(strings.Join(warns, "\n"))), hex.EncodeToString([]byte(load)))
		}
		must(os.WriteFile(filepath.Join(convertDir, c.name+".txt"), []byte(w.String()), 0o644))
		fmt.Fprintf(os.Stderr, "%s: %d inputs\n", c.name, len(inputs))
	}

	// The CLI on the samples, each copied beside akit's tests.
	must(os.RemoveAll(convertCases))
	must(os.MkdirAll(convertCases+"/golden", 0o755))
	var cases strings.Builder
	samples := []struct{ from, src, name string }{
		{"nginx", "adapter/nginx/testdata/proxy.conf", "proxy.conf"},
		{"nginx", "adapter/nginx/testdata/static.conf", "static.conf"},
		{"nginx", "adapter/nginx/testdata/full.conf", "full.conf"},
		{"nginx", "assistant/testdata/eval/migrate/nginx-docs/source.conf", "nginx-docs.conf"},
		{"nginx", "assistant/testdata/eval/migrate/nginx-shop/source.conf", "nginx-shop.conf"},
		{"nginx", "bench/configs/nginx/nginx.conf", "bench-nginx.conf"},
		{"caddyfile", "adapter/caddyfile/testdata/site.caddyfile", "site.caddyfile"},
		{"caddyfile", "adapter/caddyfile/testdata/full.caddyfile", "full.caddyfile"},
		{"caddyfile", "assistant/testdata/eval/migrate/caddy-blog/source.caddyfile", "caddy-blog.caddyfile"},
		{"caddyfile", "bench/configs/caddy/Caddyfile", "bench.caddyfile"},
	}
	for _, s := range samples {
		b, err := os.ReadFile(filepath.Join(fit, s.src))
		must(err)
		in := convertCases + "/" + s.name
		must(os.WriteFile(in, b, 0o644))
		for _, t := range targets {
			code, out, errb := run("adapt", "-from", s.from, "-c", in, "-to", t.to)
			golden := "golden/" + s.name + "." + t.ext
			must(os.WriteFile(convertCases+"/"+golden, out, 0o644))
			must(os.WriteFile(convertCases+"/"+golden+".stderr", errb, 0o644))
			fmt.Fprintf(&cases, "%s\t%s\t%s\t%d\t%s\n", in, s.from, t.to, code, golden)
		}
	}
	must(os.WriteFile(cliDir+"/convert-cases.txt", []byte(cases.String()), 0o644))
}

// fuzzSeeds returns the strings of a Go fuzz corpus.
func fuzzSeeds(dir string) []string {
	files, _ := filepath.Glob(filepath.Join(dir, "*"))
	sort.Strings(files)
	var out []string
	for _, f := range files {
		b, err := os.ReadFile(f)
		must(err)
		for _, l := range strings.Split(string(b), "\n") {
			if q, ok := strings.CutPrefix(l, "string("); ok {
				s, err := strconv.Unquote(strings.TrimSuffix(q, ")"))
				must(err)
				out = append(out, s)
			}
		}
	}
	return out
}

// testLiterals returns the string constants in the test files of dir: each literal, and each
// sum of literals.
func testLiterals(dir string) []string {
	files, _ := filepath.Glob(filepath.Join(dir, "*_test.go"))
	sort.Strings(files)
	var out []string
	fset := token.NewFileSet()
	for _, f := range files {
		file, err := parser.ParseFile(fset, f, nil, 0)
		must(err)
		ast.Inspect(file, func(n ast.Node) bool {
			e, ok := n.(ast.Expr)
			if !ok {
				return true
			}
			if s, ok := constString(e); ok {
				out = append(out, s)
				return false
			}
			return true
		})
	}
	return out
}

// constString evaluates a string literal, or a sum of them.
func constString(e ast.Expr) (string, bool) {
	switch e := e.(type) {
	case *ast.BasicLit:
		if e.Kind != token.STRING {
			return "", false
		}
		s, err := strconv.Unquote(e.Value)
		return s, err == nil
	case *ast.ParenExpr:
		return constString(e.X)
	case *ast.BinaryExpr:
		if e.Op != token.ADD {
			return "", false
		}
		a, ok1 := constString(e.X)
		b, ok2 := constString(e.Y)
		return a + b, ok1 && ok2
	}
	return "", false
}

var fence = regexp.MustCompile("(?s)```([A-Za-z]*)\n(.*?)```")

// docBlocks returns the code blocks of fit's docs written in lang.
func docBlocks(fit, lang string) []string {
	files, _ := filepath.Glob(filepath.Join(fit, "docs/*.md"))
	more, _ := filepath.Glob(filepath.Join(fit, "docs/*/*.md"))
	files = append(files, more...)
	sort.Strings(files)
	var out []string
	for _, f := range files {
		b, err := os.ReadFile(f)
		must(err)
		for _, m := range fence.FindAllSubmatch(b, -1) {
			if strings.EqualFold(string(m[1]), lang) {
				out = append(out, string(bytes.TrimLeft(m[2], "\n")))
			}
		}
	}
	return out
}
