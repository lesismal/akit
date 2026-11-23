#!/usr/bin/env bash
# Tests by reach (rules.md rule 25): the crates whose files changed since BASE (default
# origin/main, plus uncommitted changes), and the workspace crates that depend on them.
# A change to the workspace manifest, the lock file or the toolchain tests everything; a change
# to docs alone tests nothing.
set -euo pipefail
cd "$(dirname "$0")/.."
base="${1:-origin/main}"
changed=$( { git diff --name-only "$base"... 2>/dev/null || git diff --name-only "$base"; git diff --name-only HEAD; git ls-files --others --exclude-standard; } | sort -u)
if echo "$changed" | grep -qE '^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|clippy\.toml)$'; then
    exec cargo test --workspace
fi
crates=$(echo "$changed" | sed -n 's|^crates/\([^/]*\)/.*|\1|p' | sort -u)
if [ -z "$crates" ]; then
    echo "no crate changed since $base"; exit 0
fi
pkgs=()
for c in $crates; do
    [ -f "crates/$c/Cargo.toml" ] || continue
    # The crate and every workspace crate that depends on it.
    while IFS= read -r p; do pkgs+=("-p" "$p"); done < <(
        cargo tree --workspace -i "$c" -e normal,dev --prefix none --format '{p}' 2>/dev/null |
            awk '{print $1}' | grep '^akit' | sort -u)
done
[ "${#pkgs[@]}" -gt 0 ] || { echo "no crate to test"; exit 0; }
echo "testing: ${pkgs[*]}"
exec cargo test "${pkgs[@]}"
