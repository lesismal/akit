#!/bin/sh
# akit's YAML reader against fit's (decision 005): builds fit's adapter as a command (Go, with fit
# beside akit or at FIT_DIR, offline from Go's module cache), makes the corpus, has fit judge every
# case, and runs akit-config's differential test on them. The cases read differently are written
# to target/yaml-diff/akit.diff.
#
#   tools/yaml-diff/run.sh [seed] [edits]
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
fit=$(cd "${FIT_DIR:-$root/../fit}" && pwd)
out=$root/target/yaml-diff
mkdir -p "$out/src"
cp "$root/tools/yaml-diff/main.go" "$fit/go.sum" "$out/src/"
cat >"$out/src/go.mod" <<MOD
module yamldiff

go 1.24

require github.com/lesismal/fit v0.0.0

replace github.com/lesismal/fit => $fit
MOD
(cd "$out/src" && GOFLAGS=-mod=mod GOPROXY=${GOPROXY:-off} go build -o "$out/fit-yaml" .)
python3 -I "$root/tools/yaml-diff/corpus.py" "$out" "$fit" \
	"$root/tests/compat/fixtures/fit/templates/yaml" "${1:-1}" "${2:-100000}"
"$out/fit-yaml" <"$out/cases.hex" >"$out/fit.txt"
AKIT_YAML_DIFF=$out cargo test -p akit-config --lib differential_against_fit -- --ignored --nocapture
