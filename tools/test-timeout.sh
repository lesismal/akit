#!/usr/bin/env bash
# Cargo runner that puts a wall-clock limit on each test binary (docs/rules.md, rule 31).
#
#   export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=$PWD/tools/test-timeout.sh   # (or the aarch64 / apple triple)
#   cargo test --workspace --no-fail-fast -- --test-threads=1
#
# Cargo then starts every test binary through this script. A binary still running after
# AKIT_TEST_BINARY_TIMEOUT seconds (default 900) is reported as hung: a `HANG:` line, the
# binary's threads (the harness names a test's thread after the test, so the hung test shows) and
# where they wait. Then it is killed, and the run goes on
# with the next binary; this binary counts as failed (exit 124). The script only runs binaries,
# so it is harmless for `cargo run` too.
set -u
limit=${AKIT_TEST_BINARY_TIMEOUT:-900}
"$@" &
pid=$!
(
    # The trap is set before the sleep starts and ends whatever this subshell started, so a TERM
    # that comes as soon as the binary ends still ends the sleep; and the sleep holds none of the
    # binary's pipes, so it could never keep whoever reads them (rustdoc running a doctest) waiting.
    trap 'kill $(jobs -p) 2>/dev/null; exit 0' TERM
    sleep "$limit" </dev/null >/dev/null 2>&1 &
    wait $!
    kill -0 "$pid" 2>/dev/null || exit 0
    {
        echo "HANG: $(basename "$1") still running after ${limit}s (AKIT_TEST_BINARY_TIMEOUT); threads:"
        if [ -d "/proc/$pid/task" ]; then
            for t in /proc/"$pid"/task/*; do
                echo "  $(cat "$t/comm" 2>/dev/null) $(cat "$t/wchan" 2>/dev/null)"
            done | sort | uniq -c
        else
            ps -M -p "$pid" 2>/dev/null || ps -p "$pid" -o pid,etime,command
        fi
        echo "HANG: killing $(basename "$1")"
    } >&2
    kill -KILL "$pid" 2>/dev/null
    touch "${TMPDIR:-/tmp}/akit-test-hang.$pid"
) &
watchdog=$!
wait "$pid"
rc=$?
kill "$watchdog" 2>/dev/null
wait "$watchdog" 2>/dev/null
if [ -e "${TMPDIR:-/tmp}/akit-test-hang.$pid" ]; then
    rm -f "${TMPDIR:-/tmp}/akit-test-hang.$pid"
    exit 124
fi
exit "$rc"
