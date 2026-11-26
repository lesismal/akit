// fit's `adapt` and its writers as a command, for the golden files of akit's `akit adapt` and
// `akit fmt` (I73, tools/adapt-golden/run.sh). Run from akit's root with fit's examples' secrets
// set; it writes:
//
//   - crates/akit/tests/testdata/adapt/cases.txt and golden/: `fit adapt -c <input> -to <to>`
//     on akit's copies of fit's templates and examples (tests/compat/fixtures) and fit's
//     cli/testdata configs (copied to crates/akit/tests/testdata/adapt/fit), each output as fit
//     writes it. A line of cases.txt: input, to, exit code, golden file, tab-separated.
//   - crates/akit-config/testdata/adapt-writers.txt: fit's YAML writer (adapter/yaml.FromJSON)
//     and .conf writer (adapter/fitconf.FromJSON) on a corpus of JSON documents: the outputs of
//     the cases above, documents of fit's writer tests, and documents built from strings and
//     numbers each writer treats specially, placed as keys and values. A line: the document,
//     the YAML, the .conf, tab-separated; a document or an output hex-encoded, an output as
//     "OK <hex>" or "ERR <hex of the message>".
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"sort"
	"strings"

	_ "github.com/lesismal/fit/app/standard"
	"github.com/lesismal/fit/adapter/fitconf"
	"github.com/lesismal/fit/adapter/yaml"
	"github.com/lesismal/fit/cli"
	_ "github.com/lesismal/fit/modules/standard"
	_ "github.com/lesismal/fit/provider/standard"
)

const (
	cliDir     = "crates/akit/tests/testdata/adapt"
	writerFile = "crates/akit-config/testdata/adapt-writers.txt"
)

// targets are fit's -to names, each with akit's name for it (fit's "fit" is akit's "akit",
// decision 037) and the golden file's extension.
var targets = []struct{ to, akit, ext string }{
	{"json", "json", "json"}, {"yaml", "yaml", "yaml"}, {"fit", "akit", "conf"},
}

func must(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// inputs are the configs the CLI cases convert: files and directories.
func inputs() []string {
	t := "tests/compat/fixtures/fit/templates"
	in := []string{
		t + "/json/fit.json", t + "/yaml/fit.yaml", t + "/conf/fit.conf",
		t + "/json/conf.d", t + "/yaml/conf.d", t + "/conf/conf.d",
	}
	fit, _ := filepath.Glob(cliDir + "/fit/*")
	sort.Strings(fit)
	in = append(in, fit...)
	ex, _ := filepath.Glob("tests/compat/fixtures/fit-examples/*/*.conf")
	sort.Strings(ex)
	return append(in, ex...)
}

func run(args ...string) (int, []byte, []byte) {
	var out, errb bytes.Buffer
	code := cli.Main(args, &out, &errb)
	return code, out.Bytes(), errb.Bytes()
}

func main() {
	must(os.RemoveAll(cliDir + "/golden"))
	must(os.MkdirAll(cliDir+"/golden", 0o755))
	var cases strings.Builder
	var docs [][]byte
	for _, in := range inputs() {
		name := strings.NewReplacer("tests/compat/fixtures/", "", cliDir+"/", "", "/", "__").Replace(in)
		for _, t := range targets {
			code, out, errb := run("adapt", "-c", in, "-to", t.to)
			golden := "-"
			if code == 0 {
				golden = "golden/" + name + "." + t.ext
				must(os.WriteFile(cliDir+"/"+golden, out, 0o644))
				if t.to == "json" {
					docs = append(docs, out)
				}
			} else {
				fmt.Fprintf(os.Stderr, "%s -to %s: %d %s", in, t.to, code, errb)
			}
			fmt.Fprintf(&cases, "%s\t%s\t%d\t%s\n", in, t.akit, code, golden)
		}
	}
	must(os.WriteFile(cliDir+"/cases.txt", []byte(cases.String()), 0o644))

	docs = append(docs, corpus()...)
	var w strings.Builder
	for _, d := range docs {
		y, yerr := yaml.FromJSON(d)
		c, cerr := fitconf.FromJSON(d)
		fmt.Fprintf(&w, "%s\t%s\t%s\n", hex.EncodeToString(d), verdict(y, yerr), verdict(c, cerr))
	}
	must(os.WriteFile(writerFile, []byte(w.String()), 0o644))
	fmt.Fprintf(os.Stderr, "%d CLI inputs, %d writer documents\n", len(inputs()), len(docs))
	convertGolden(os.Getenv("FIT_DIR"))
}

func verdict(out []byte, err error) string {
	if err != nil {
		return "ERR " + hex.EncodeToString([]byte(err.Error()))
	}
	return "OK " + hex.EncodeToString(out)
}

// strs are strings the writers treat specially: the YAML writer's plain, quoted and block
// styles, its resolver's booleans, numbers and timestamps, and the .conf writer's words.
func strs() []string {
	s := []string{
		"", " ", "a", "a b", " a", "a ", "a  b", "a\n", "a\nb", "\n", "\na", "a\n\n", "a\n ", "a \nb",
		"a\n b", " a\nb", "a\nb ", "\n\n", "\t", "a\tb", "\ta", "a\t", "a\r", "a\r\nb", "\r",
		"true", "True", "TRUE", "tRue", "false", "False", "yes", "no", "on", "off", "y", "n", "Y",
		"null", "Null", "NULL", "nUll", "~", "<<", "<", "<<<", "---", "--- a", "---a", "...", "....",
		"-", "--", "- ", "-a", "- a", "?", "? a", "?a", ":", ": a", ":a", "a:", "a: b", "a:b", "a :b",
		"#", "#a", "a #b", "a#b", "a\t#b", ",", "a,b", "[", "]", "{", "}", "{}", "[]", "a[b]",
		"a{b}", "&a", "*a", "!a", "|", ">", "'", "\"", "%", "@", "`", "a'b", "a\"b", "a\\b", "'a'",
		"0", "1", "-1", "+1", "1.5", "-1.5", "1e3", "1E3", "1e+3", "-1e-3", "1E400", "-1e400", ".5",
		"5.", "-.5", "+.5", ".", "..", ".e5", ".5e999", "1.", "1.2.3", ".inf", ".Inf", ".INF", ".nan",
		".NaN", "-.inf", "+.inf", "+.Inf", "inf", "nan", "-inf", "0x1F", "0X1F", "0x", "0xg",
		"-0x1F", "+0x1F", "0o17", "0O17", "0o8", "0b101", "0B101", "0b2", "-0b11", "-0o17", "017",
		"08", "09.5", "00", "-0", "+0", "1_000", "_1", "1__0", "0b1_0", "1_0.5",
		"0b" + strings.Repeat("1", 64), "0b" + strings.Repeat("1", 65), "0o" + strings.Repeat("7", 22),
		"-0b" + strings.Repeat("1", 64), "99999999999999999999", "18446744073709551615",
		"18446744073709551616", "9223372036854775807", "9223372036854775808",
		"-9223372036854775808", "-9223372036854775809", "1e308", "2e308", "4.9e-324", "1e-400",
		"2024-01-02", "2024-1-2", "2024-1-02", "2024-01-2", "2024-02-30", "2023-02-29",
		"2024-02-29", "2100-02-29", "2000-02-29", "2024-13-01", "2024-00-01", "2024-01-00",
		"2024-01-32", "2024-001-01", "202-01-01", "20240-01-01", "2024-01-02T10:00:00Z",
		"2024-01-02t10:00:00Z", "2024-01-02T10:00:00+08:00", "2024-01-02T10:00:00-0800",
		"2024-01-02T1:2:3Z", "2024-01-02T10:00:00.123456789Z", "2024-01-02T10:00:00.1234567891Z",
		"2024-01-02T10:00:00", "2024-01-02T25:00:00Z", "2024-01-02T10:60:00Z",
		"2024-01-02T10:00:60Z", "2024-01-02 10:00:00", "2024-01-02 10:00:00.5",
		"2024-01-02 10:00", "2024-01-02  10:00:00", "2024-01-02 10:00:00Z", "2024-01-02x",
		"2024-01-02T10:00:00.Z", "2024-01-02T10:00:00,5Z", "2024-01-02T10:00:00+24:00",
		"2024-01-02T10:00:00+08:60", "2024-01-02T10:00:00+8:00", "2024-01-02T10:00:00z",
		"é", "日本", "😀", "a😀", "\u0085", "a\u0085b", "a\u0085", "\u2028", "a\u2028b",
		"\u2029", "\ufeff", "\ufeffa", "a\ufeff", "\u00a0", "a\u00a0", "\u007f", "\u0001", "a\u0000b",
		"\u001b", "\ufffe", "\uffff", "\ufffd", "\ue000", "\ud7ff", "\u0080", "\u009f", "\u00e0",
		"{$HOME}", "{$X} `y`", "a{$", "{$", "a;", ";a", "a;b", ";", "x;", "{path}", "a}", "}a",
		"/a/*", ":8080", "http/1.1", "127.0.0.1:9000", "a.example", "*.example.com", "5s",
		"1GiB", "50%", "C:\\x", "a=b", "$a", "a$", "(a)", "a|b",
		strings.Repeat("x", 128), strings.Repeat("x", 129), strings.Repeat("é", 64),
		strings.Repeat("é", 65), strings.Repeat("ab ", 40), "long " + strings.Repeat("y", 100),
	}
	return s
}

// nums are JSON numbers as written.
var nums = []string{
	"0", "-0", "1", "-1", "1.5", "200.0", "1e3", "1E3", "1e+3", "1e-3", "-1.5e10", "1E400",
	"0.0", "99999999999999999999", "18446744073709551616", "9223372036854775807", "1073741824",
	"1048576", "1024", "1000", "1099511627776", "3221225472", "1536", "5e2",
}

func doc(v any) []byte {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	must(enc.Encode(v))
	return bytes.TrimSpace(b.Bytes())
}

// ord is an object with its members in order.
type ord []kv
type kv struct {
	k string
	v any
}

func (o ord) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, m := range o {
		if i > 0 {
			b.WriteByte(',')
		}
		b.Write(doc(m.k))
		b.WriteByte(':')
		b.Write(doc(m.v))
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

func corpus() [][]byte {
	var out [][]byte
	add := func(s string) { out = append(out, []byte(s)) }
	// fit's writer tests.
	for _, s := range []string{
		`{"listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]},
		{"name": "tls", "address": ":8443", "tls": {"certificates": [{"cert_file": "c.pem", "key_file": "k.pem"}], "acme": ["a.example"]}}],
	"upstreams": [{"name": "app", "addresses": ["10.0.0.1:80", "10.0.0.2:80"]},
		{"name": "big", "addresses": ["10.0.0.3:80"], "max_conns": 8192, "max_pending": 8192, "max_idle_conns": 8192, "connect_timeout": "5s", "max_fails": 0}],
	"routes": [{"name": "a", "match": {"path": ["/a"]},
		"middleware": [{"module": "cache", "params": {"max_bytes": 1073741824, "max_entry_bytes": 1000}}],
		"handler": {"module": "proxy", "params": {"upstream": "app"}}},
		{"name": "b", "handler": {"module": "respond", "params": {"body": "a body far too long to be written in a block on one line, past eighty"}}}]}`,
		`{"admin": null, "bogus": {"n": null, "t": true, "f": false, "s": "x", "l": [1, "2"], "o": {}}}`,
		`{"limits": {"max_conns": "x"}}`,
		`{"routes": [{"handler": {"module": "nope", "params": {"a": 1, "b": {"c": "d"}}}}]}`,
		`{"admin": 1}`, `{"listeners": 2}`, `{"routes": [{"match": {"header": 1}}]}`,
		`[]`, `{"bogus": [1]}`, `{"bogus": [{}, 1]}`, `{"bogus": {"a": [1]}}`,
		`{"listeners": [1]}`, `{"listeners": ["x"]}`, `{"listeners": [true]}`, `{"listeners": [null]}`,
		`{"listeners": [[]]}`, `{"listeners": [{"name": "a", "bogus": [1]}]}`,
		`{"listeners": [{"name": "a", "protocols": [{}]}]}`, `{"listeners": [{"name": "a", "address": {}}]}`,
		`{"routes": [{"match": {"header": {"X": [{}]}}, "handler": {"module": "respond"}}]}`,
		`{"routes": [{"middleware": [1], "handler": {"module": "respond"}}]}`,
		`{"routes": [{"handler": {}}]}`, `{"routes": [{"handler": {"module": "respond", "x": 1}}]}`,
		`{"routes": [{"handler": {"module": "respond", "params": 1}}]}`,
		`{"routes": [{"handler": {"module": "nope", "params": {"a": [1]}}}]}`,
		`{"routes": [{"handler": {"module": "elsewhere", "params": {"x": [1]}}}]}`,
		`{"k": "<<", "<<": "x", "s": "a\nb", "n": null, "b": false, "f": 1.5}`,
		`1`, `"s"`, `null`, `true`, `[1, [2, [3]], {}, [], {"a": []}]`, `{}`,
		`{"a": [[]], "b": [{}], "c": [[1, 2], [[]]], "d": [{"x": [{"y": {}}]}], "e": {"f": {"g": []}}}`,
		`{"listeners": [{"name": "a", "address": ":1", "protocols": []}]}`,
		`{"listeners": [{"name": "a", "address": ":1", "protocols": ["h3"], "tls": {"certificates": [{"cert_file": "a", "key_file": "b"}, {"cert_file": "c", "key_file": "d"}]}}]}`,
		`{"listeners": [{"name": "a", "tls": {"certificates": [{"cert_file": "a", "key_file": "b"}]}}]}`,
		`{"listeners": [{"name": "a", "tls": {"certificates": [{"cert_file": "a", "key_file": "b", "x": 1}]}}]}`,
		`{"listeners": [{"name": "a", "tls": {"certificates": [{"cert_file": 1, "key_file": "b"}]}}]}`,
		`{"listeners": [{"address": ":1"}], "upstreams": [{"name": "u", "addresses": []}, {"name": 7, "addresses": ["a"]}, {"name": "v", "addresses": "x"}, {"name": "w", "addresses": ["a", 1]}]}`,
		`{"listeners": [{"name": "a", "address": 1, "protocols": ["x"]}, {"name": "b", "protocols": ["x"], "address": ":1"}]}`,
		`{"routes": [{"handler": {"module": "respond"}, "middleware": [{"module": "headers"}, {"module": "cache", "params": {}}]}]}`,
		`{"routes": [{"handler": {"module": "respond", "params": {"headers": {"A": "b", "C": "d e"}}}}], "limits": {"max_body_bytes": 1048576, "max_header_bytes": 3}}`,
		`{"limits": {"max_body_bytes": -1024, "max_header_bytes": 1.5, "max_conns": 1024}}`,
		`{"log": {"loggers": {"a": {"level": "info"}, "b": {}}}}`,
		`{"routes": [{"match": {"header": {"X-A": ["1", "2"], "X-B": ["true"], "X-C": []}}, "handler": {"module": "respond"}}]}`,
		`{"routes": [{"match": {"path": ["/a", "/b"], "method": ["GET"]}, "handler": {"module": "static", "params": {"root": "/srv"}}}]}`,
		`{"upstreams": [{"name": "a", "addresses": ["x"], "weights": [1, 2], "health_check": {"expect_status": [200, 204]}}]}`,
		`{"x": {"y": {"z": [1, 2, [3]]}}}`, `{"x": [true, false]}`, `{"x": false}`, `{"x": true}`,
	} {
		add(s)
	}
	rnd := rand.New(rand.NewSource(73))
	ss := strs()
	for _, s := range ss {
		add(string(doc(ord{{"k", s}})))
		add(string(doc(ord{{s, "v"}})))
		add(string(doc(ord{{"l", []any{s, s}}, {"m", ord{{"n", []any{ord{{s, s}}}}}}})))
		add(string(doc(ord{{"admin", ord{{"address", s}}}})))
		add(string(doc(ord{{"listeners", []any{ord{{"name", s}, {"address", s}, {"protocols", []any{s, s}}}}}})))
		add(string(doc(ord{{"routes", []any{ord{{"handler", ord{{"module", "respond"}, {"params", ord{{"body", s}, {"headers", ord{{s, s}}}}}}}}}}})))
		add(string(doc(ord{{"bogus", ord{{s, s}, {"l", []any{s, s, s}}}}})))
		add(string(doc(ord{{"l", []any{[]any{s}, ord{{s, []any{s}}}}}})))
	}
	for _, n := range nums {
		add(`{"k": ` + n + `, "l": [` + n + `], "bogus": {"a": ` + n + `, "b": [` + n + `, ` + n + `]}}`)
		add(`{"limits": {"max_body_bytes": ` + n + `}, "admin": {"address": ` + n + `}}`)
	}
	// Random trees of the strings and numbers.
	var tree func(depth int) any
	tree = func(depth int) any {
		switch k := rnd.Intn(8); {
		case k < 2 && depth < 4:
			var o ord
			seen := map[string]bool{}
			for i := rnd.Intn(4); i > 0; i-- {
				key := ss[rnd.Intn(len(ss))]
				if !seen[key] {
					seen[key] = true
					o = append(o, kv{key, tree(depth + 1)})
				}
			}
			if o == nil {
				return map[string]any{}
			}
			return o
		case k < 4 && depth < 4:
			a := []any{}
			for i := rnd.Intn(4); i > 0; i-- {
				a = append(a, tree(depth+1))
			}
			return a
		case k == 4:
			return json.Number(nums[rnd.Intn(len(nums))])
		case k == 5:
			return rnd.Intn(2) == 0
		default:
			return ss[rnd.Intn(len(ss))]
		}
	}
	for i := 0; i < 1500; i++ {
		var o ord
		seen := map[string]bool{}
		for j := 1 + rnd.Intn(3); j > 0; j-- {
			key := []string{"bogus", "listeners", "routes", "admin", "x", ss[rnd.Intn(len(ss))]}[rnd.Intn(6)]
			if !seen[key] {
				seen[key] = true
				o = append(o, kv{key, tree(0)})
			}
		}
		add(string(doc(o)))
	}
	return out
}
