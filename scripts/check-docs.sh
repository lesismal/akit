#!/usr/bin/env bash
# Every document is bilingual (rules.md rule 26): X.md has X.zh-CN.md, each links the other
# under its title. CLAUDE.md is the one exception (instructions for sessions, not a document).
# Copies of other projects' files (tests/compat/fixtures) are not akit documents.
# Reports benchmark runs write (bench/results) are generated, as fit's benchreport writes them.
set -euo pipefail
cd "$(dirname "$0")/.."
bad=0
while IFS= read -r f; do
    case "$f" in
        *.zh-CN.md | ./CLAUDE.md | ./target/*) continue ;;
    esac
    base=$(basename "$f" .md)
    twin="${f%.md}.zh-CN.md"
    if [ ! -f "$twin" ]; then
        echo "missing Chinese twin: $twin"; bad=1; continue
    fi
    for doc in "$f" "$twin"; do
        if ! head -n 5 "$doc" | grep -q "(${base}.md)" || ! head -n 5 "$doc" | grep -q "(${base}.zh-CN.md)"; then
            echo "no language links under the title: $doc"; bad=1
        fi
    done
done < <(find . -name '*.md' -not -path './target/*' -not -path './.git/*' -not -path './.claude/*' -not -path './fuzz/target/*' -not -path './tests/compat/fixtures/*' -not -path './bench/results/*' | sort)
while IFS= read -r twin; do
    [ -f "${twin%.zh-CN.md}.md" ] || { echo "missing English original of $twin"; bad=1; }
done < <(find . -name '*.zh-CN.md' -not -path './target/*' -not -path './.git/*' -not -path './.claude/*' -not -path './fuzz/target/*' -not -path './tests/compat/fixtures/*' -not -path './bench/results/*')
if [ "$bad" -eq 0 ]; then echo "docs: every document has its twin"; fi
exit "$bad"
