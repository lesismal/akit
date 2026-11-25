#!/usr/bin/env bash
# Runs every fuzz target for SECS seconds (default 300), PAR at a time (default: CPUs), each on a
# scratch corpus seeded from fuzz/corpus/<target>: new inputs land in fuzz/corpus-new/<target>,
# crashes, leaks and timeouts in fuzz/artifacts/<target>, logs in fuzz/logs (all gitignored).
# Needs nightly and cargo-fuzz. Exits 1 when any target failed or left an artifact.
#
#   fuzz/run-all.sh [SECS] [PAR] [target...]
#
# Afterwards, `fuzz/run-all.sh merge [target...]` folds corpus-new into fuzz/corpus, minimized,
# the named seeds (hand-written, regressions) kept.
set -u
cd "$(dirname "$0")"
TOOLCHAIN=${TOOLCHAIN:-+nightly}
cpus() { nproc 2>/dev/null || sysctl -n hw.ncpu; }
if [ "${1:-}" = merge ]; then
    # Named seeds (hand-written ones, regressions) stay as they are; the rest of the corpus
    # (inputs named by their hash, imports) and corpus-new are reduced to the fewest inputs that
    # keep their features.
    shift
    mkdir -p logs merge
    for t in ${@:-$(cargo $TOOLCHAIN fuzz list)}; do
        [ -n "$t" ] && [ -d "corpus/$t" ] || continue
        mkdir -p "merge/$t/min" "merge/$t/rest" "corpus-new/$t"
        for f in "corpus/$t"/*; do
            b=$(basename "$f")
            if [[ $b =~ ^[0-9a-f]{40}$ || $b == fit-* || $b == akio-* || $b == table-* ]]; then
                cp "$f" "merge/$t/rest/"
            else
                cp "$f" "merge/$t/min/"
            fi
        done
        cargo $TOOLCHAIN fuzz run "$t" "merge/$t/min" "merge/$t/rest" "corpus-new/$t" -- \
            -set_cover_merge=1 -rss_limit_mb=4096 -timeout=10 >"logs/$t.merge.log" 2>&1 || {
            echo "$t: merge failed, corpus kept" >&2
            continue
        }
        mv "corpus/$t" "merge/$t/old" && mv "merge/$t/min" "corpus/$t"
        echo "$t: $(ls "corpus/$t" | wc -l | tr -d ' ') files"
    done
    rm -rf merge
    exit 0
fi
SECS=${1:-300}
PAR=${2:-$(cpus)}
shift 2 2>/dev/null || shift $#
TARGETS=${*:-$(cargo $TOOLCHAIN fuzz list)}
cargo $TOOLCHAIN fuzz build >/dev/null 2>&1 || cargo $TOOLCHAIN fuzz build || exit 1
mkdir -p logs corpus-new
run() {
    t=$1
    mkdir -p "corpus-new/$t"
    dict=()
    [ -f "dict/$t.dict" ] && dict=(-dict="dict/$t.dict")
    cargo $TOOLCHAIN fuzz run "$t" "corpus-new/$t" "corpus/$t" -- \
        -max_total_time="$SECS" -rss_limit_mb=2048 -timeout=10 -max_len=16384 \
        -print_final_stats=1 "${dict[@]}" >"logs/$t.log" 2>&1
    rc=$?
    execs=$(grep -m1 'stat::number_of_executed_units' "logs/$t.log" | awk '{print $2}')
    echo "$t rc=$rc execs=${execs:-?}"
}
export -f run
export SECS TOOLCHAIN
printf '%s\n' $TARGETS | xargs -P "$PAR" -I{} bash -c 'run {}' | tee logs/summary.txt
if grep -qv ' rc=0 ' logs/summary.txt || [ -n "$(find artifacts -type f 2>/dev/null | head -1)" ]; then
    echo "FAILED: see fuzz/logs and fuzz/artifacts" >&2
    exit 1
fi
echo "DONE"
