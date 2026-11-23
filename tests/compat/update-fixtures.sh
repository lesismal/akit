#!/usr/bin/env bash
# Refreshes the pinned fit fixtures of the compatibility gate (docs/compatibility.md) from the
# given commits of fit and fit-examples, then regenerates the field inventory and its tables.
#
#   tests/compat/update-fixtures.sh <fit-commit> <fit-examples-commit>
#
# FIT_REPO and FIT_EXAMPLES_REPO name the repositories to clone (a URL or a local path); they
# default to GitHub. Moving the pin is a reviewed change: commit the result on its own, with the
# diff of tests/compat/fit-fields.txt read in review.
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <fit-commit> <fit-examples-commit>" >&2
    exit 2
fi
fit_rev=$1
examples_rev=$2
fit_repo=${FIT_REPO:-https://github.com/lesismal/fit.git}
examples_repo=${FIT_EXAMPLES_REPO:-https://github.com/lesismal/fit-examples.git}

root=$(cd "$(dirname "$0")/../.." && pwd)
fixtures=$root/tests/compat/fixtures
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() { # repo rev dir: a checkout of rev, read only
    git clone -q --no-checkout "$1" "$3"
    git -C "$3" -c advice.detachedHead=false checkout -q "$2"
    git -C "$3" rev-parse HEAD
}
fit_hash=$(fetch "$fit_repo" "$fit_rev" "$work/fit")
examples_hash=$(fetch "$examples_repo" "$examples_rev" "$work/fit-examples")

rm -rf "$fixtures/fit" "$fixtures/fit-examples"
mkdir -p "$fixtures/fit/schema" "$fixtures/fit-examples"

# fit: the templates in all three formats (single files and conf.d directories), the JSON
# Schema the field inventory is generated from, and the licence.
cp -R "$work/fit/templates" "$fixtures/fit/templates"
cp "$work/fit/schema/fit.schema.json" "$fixtures/fit/schema/fit.schema.json"
cp "$work/fit/LICENSE" "$fixtures/fit/LICENSE"

# fit-examples: every example's config, as fit-examples' own test finds them (*/fit.conf), and
# the licence. Other files of an example (compose files, Kubernetes manifests, data) are not
# fit configs and are left out.
(cd "$work/fit-examples" && for f in */fit.conf; do
    mkdir -p "$fixtures/fit-examples/$(dirname "$f")"
    cp "$f" "$fixtures/fit-examples/$f"
done)
cp "$work/fit-examples/LICENSE" "$fixtures/fit-examples/LICENSE"

cat >"$fixtures/PINS" <<EOF
# The commits the fixtures were copied from; written by tests/compat/update-fixtures.sh.
fit=$fit_hash
fit-examples=$examples_hash
EOF

# The pins in SOURCE.md and its twin, on their marked lines.
for doc in "$root/tests/compat/SOURCE.md" "$root/tests/compat/SOURCE.zh-CN.md"; do
    [ -f "$doc" ] || continue
    sed -i.bak \
        -e "s/^\(| fit | \`\)[0-9a-f]*\(\` |.*\)$/\1$fit_hash\2/" \
        -e "s/^\(| fit-examples | \`\)[0-9a-f]*\(\` |.*\)$/\1$examples_hash\2/" "$doc"
    rm -f "$doc.bak"
done

python3 -I "$root/tools/compat-inventory.py" --root "$root"

echo "fit $fit_hash, fit-examples $examples_hash"
echo "templates: $(find "$fixtures/fit/templates" -type f \( -name '*.conf' -o -name '*.json' -o -name '*.yaml' \) | wc -l | tr -d ' ') files"
echo "examples:  $(find "$fixtures/fit-examples" -name fit.conf | wc -l | tr -d ' ') configs"
echo "fields:    $(grep -cv '^#' "$root/tests/compat/fit-fields.txt")"
