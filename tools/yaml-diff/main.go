// fit's YAML adapter (adapter/yaml) as a command, for akit's differential check of its YAML
// reader (decision 005, tools/yaml-diff/run.sh). Reads hex-encoded YAML documents, one a line,
// and prints fit's verdict on each: "OK <canonical JSON>" or "ERR <message>". The canonical JSON
// writes strings and keys hex-encoded (s<hex>, k<hex>:), numbers as fit writes them.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	y "github.com/lesismal/fit/adapter/yaml"
)

func walk(dec *json.Decoder, b *strings.Builder) {
	t, _ := dec.Token()
	switch t := t.(type) {
	case json.Delim:
		if t == '{' {
			b.WriteString("{")
			first := true
			for dec.More() {
				if !first {
					b.WriteString(",")
				}
				first = false
				k, _ := dec.Token()
				b.WriteString("k" + hex.EncodeToString([]byte(k.(string))) + ":")
				walk(dec, b)
			}
			dec.Token()
			b.WriteString("}")
		} else {
			b.WriteString("[")
			first := true
			for dec.More() {
				if !first {
					b.WriteString(",")
				}
				first = false
				walk(dec, b)
			}
			dec.Token()
			b.WriteString("]")
		}
	case string:
		b.WriteString("s" + hex.EncodeToString([]byte(t)))
	case json.Number:
		b.WriteString(string(t))
	case bool:
		fmt.Fprint(b, t)
	case nil:
		b.WriteString("null")
	}
}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<28)
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	for sc.Scan() {
		data, _ := hex.DecodeString(sc.Text())
		func() {
			defer func() {
				if r := recover(); r != nil {
					fmt.Fprintln(w, "ERR panic", strings.ReplaceAll(fmt.Sprint(r), "\n", " "))
				}
			}()
			r, err := y.Adapt(data)
			if err != nil {
				fmt.Fprintln(w, "ERR", strings.ReplaceAll(err.Error(), "\n", " "))
				return
			}
			dec := json.NewDecoder(bytes.NewReader(r.JSON))
			dec.UseNumber()
			var b strings.Builder
			walk(dec, &b)
			fmt.Fprintln(w, "OK", b.String())
		}()
	}
}
