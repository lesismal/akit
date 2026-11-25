#!/bin/sh
# stampede.sh runs the stampede check on the benchmark host (M5's exit criterion, I76): many
# clients ask a cache for one cold key at once, the origin slow to answer, and the origin counts
# the fetches that reach it; for nginx with proxy_cache_lock on, fit, akit, and akit with
# disable_lock as the control (bench/stampede/run.sh says how). It waits for the check to end.
#
#   bench/stampede.sh [options] [REF]   akit at REF (default: work, the working tree)
#
# Options:
#
#   -clients N   clients asking for each key at once     (default: 1000)
#   -keys N      cold keys a server and protocol          (default: 3)
#   -delay S     seconds the origin takes to answer       (default: 1)
#
# The check takes /root/bench.lock alone, through the host's quiet.sh: it is short (about two
# minutes) but puts a thousand connections on the server's CPUs at once.
set -eu
. "$(cd "$(dirname "$0")" && pwd)/lib.sh"

clients=1000
keys=3
delay=1
while [ $# -gt 1 ] || { [ $# -eq 1 ] && case "$1" in -*) true ;; *) false ;; esac; }; do
	case "$1" in
	-clients) clients=$2 ;;
	-keys) keys=$2 ;;
	-delay) delay=$2 ;;
	*) die "unknown option $1" ;;
	esac
	shift 2
done
ref=${1:-work}

id=stampede-$(date +%Y%m%d-%H%M%S)
stage_fit
desc=$(stage_akit "$ref" "$id")
dir="$BENCH_DIR/runs/$id"
COPYFILE_DISABLE=1 tar --no-xattrs -cf - -C "$here/stampede" . | $BENCH_SSH "mkdir -p $dir && tar xf - -C $dir"
echo "stampede: running $id (akit $desc) on the benchmark host"
out="$here/results/$id"
mkdir -p "$out"
echo "$desc" > "$out/ref"
echo "clients=$clients keys=$keys delay=$delay fit=$fit_rev" > "$out/params"
$BENCH_SSH "cd $dir && $QUIET run ./run.sh $fit_dir/bin/fit akit-bench:$id $clients $keys $delay" 2>&1 |
	grep -v '^stopped\|^restored' | tee "$out/results.txt"
# Rule 29: what the check made goes when it ends.
$BENCH_SSH "docker rmi akit-bench:$id >/dev/null 2>&1; rm -rf $dir $BENCH_DIR/src-$id"
echo "stampede: results in bench/results/$id"
