#!/bin/sh
# Builds the test plugin of akit's WASM ABI (decision 037), crates/akit-plugin/tests/guests/
# headers-akit.wasm.gz, from fit's: fit's guest package (plugin/wasm/guest) and test plugin
# (plugin/wasm/testdata/headers) at the fit commit the compatibility gate pins
# (tests/compat/fixtures/PINS), with the ABI's names renamed as akit names them, and nothing else
# changed:
#
#   //go:wasmexport fit_<name>        ->  //go:wasmexport akit_<name>   (fit_on_request, fit_sync_*)
#   //go:wasmimport fit <name>        ->  //go:wasmimport akit <name>   (the host module)
#   //go:wasmimport fit_sync <name>   ->  //go:wasmimport akit_sync <name>
#
# The build is reproducible: Go 1.27.3 (fit's go.mod), GOOS=wasip1 GOARCH=wasm, -buildmode=c-shared
# -trimpath, gzip -9 -n; the guest package needs nothing but Go's standard library.
#
#   tools/plugin-guest.sh            # fit beside akit's checkout, or at FIT_DIR
#   tools/plugin-guest.sh --check    # build it again and compare with the committed file
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
common=$(cd "$root" && cd "$(git rev-parse --git-common-dir)/.." && pwd)
fit=$(cd "${FIT_DIR:-$common/../fit}" && pwd)
commit=$(sed -n 's/^fit=//p' "$root/tests/compat/fixtures/PINS")
out=$root/crates/akit-plugin/tests/guests/headers-akit.wasm.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git -C "$fit" archive "$commit" plugin/wasm/guest plugin/wasm/testdata/headers | tar xf - -C "$tmp"
printf 'module github.com/lesismal/fit\n\ngo 1.27.3\n' >"$tmp/go.mod"
for f in "$tmp"/plugin/wasm/guest/*.go; do
	sed -e 's#^//go:wasmexport fit_#//go:wasmexport akit_#' \
		-e 's#^//go:wasmimport fit #//go:wasmimport akit #' \
		-e 's#^//go:wasmimport fit_sync #//go:wasmimport akit_sync #' "$f" >"$f.new"
	mv "$f.new" "$f"
done
if grep -n '^//go:wasm[a-z]* fit' "$tmp"/plugin/wasm/guest/*.go; then
	echo "plugin-guest: fit's guest has names the renames above do not cover" >&2
	exit 1
fi
(cd "$tmp/plugin/wasm/testdata/headers" &&
	GOOS=wasip1 GOARCH=wasm CGO_ENABLED=0 GOWORK=off GOFLAGS=-mod=mod GOPROXY=off \
		go build -trimpath -buildmode=c-shared -o "$tmp/headers.wasm" .)
gzip -9 -n -c "$tmp/headers.wasm" >"$tmp/headers-akit.wasm.gz"
if [ "${1:-}" = --check ]; then
	if cmp -s "$tmp/headers-akit.wasm.gz" "$out"; then
		echo "plugin-guest: $(basename "$out") is fit $(echo "$commit" | cut -c1-8)'s, renamed"
	else
		echo "plugin-guest: $(basename "$out") differs from the build ($(go version))" >&2
		exit 1
	fi
else
	cp "$tmp/headers-akit.wasm.gz" "$out"
	echo "plugin-guest: wrote $(basename "$out") from fit $(echo "$commit" | cut -c1-8) ($(go version))"
fi
