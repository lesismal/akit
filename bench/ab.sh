#!/bin/sh
# ab.sh compares two builds of akit on the benchmark host, in the same run (docs/rules.md, rule
# 24: a hot-path change): akit alone, only the scenarios and protocols a change touches, short
# runs, rounds that alternate A and B, without waiting for it. As fit's bench/ab.sh.
#
#   bench/ab.sh [options] A B     start a comparison; prints its id and returns
#   bench/ab.sh -report ID        fetch what has finished and summarize it
#   bench/ab.sh -wait ID          wait for it to finish, then summarize
#
# A and B are git refs of akit (main, HEAD, a commit) or "work", the working tree. Options:
#
#   -scenarios LIST   fixed                         (default: fixed)
#   -protocols LIST   http/1.1,h2c,https/1.1,h2     (default: all four)
#   -rounds N         rounds of A then B            (default: 3)
#   -duration D       each measured run             (default: 10s)
#   -warmup D         unmeasured load before each   (default: 2s)
#   -args FLAGS       more fitbench flags for every run, as '-connections 500'
#
# Every round runs under the host's /root/bench.lock alone (quiet.sh), held for the whole
# comparison, so A and B see the same host.
set -eu
. "$(cd "$(dirname "$0")" && pwd)/lib.sh"
local_dir=${AB_LOCAL_DIR:-${TMPDIR:-/tmp}/akit-ab}

report() {
	id=$1
	mkdir -p "$local_dir/$id"
	$BENCH_SSH "cd $BENCH_DIR/ab/$id && tar cf - a.ref b.ref params status done *.json 2>/dev/null" |
		tar xf - -C "$local_dir/$id" 2>/dev/null || true
	n=$(wc -l < "$local_dir/$id/status" 2>/dev/null | tr -d ' ' || echo 0)
	echo "ab: $id: ${n:-0} runs finished$([ -e "$local_dir/$id/done" ] && echo ', done' || echo ', still running')"
	python3 "$here/abreport.py" "$local_dir/$id"
}

case "${1:-}" in
-report)
	report "$2"
	exit 0
	;;
-wait)
	until $BENCH_SSH "test -e $BENCH_DIR/ab/$2/done"; do sleep 30; done
	report "$2"
	exit 0
	;;
esac

scenarios=fixed
protocols=http/1.1,h2c,https/1.1,h2
rounds=3
duration=10s
warmup=2s
args=
while [ $# -gt 2 ]; do
	case "$1" in
	-scenarios) scenarios=$2 ;;
	-protocols) protocols=$2 ;;
	-rounds) rounds=$2 ;;
	-duration) duration=$2 ;;
	-warmup) warmup=$2 ;;
	-args) args=$2 ;;
	*) die "unknown option $1" ;;
	esac
	shift 2
done
[ $# -eq 2 ] || die "usage: bench/ab.sh [options] A B"

id=ab-$(date +%Y%m%d-%H%M%S)
stage_fit
desc_a=$(stage_akit "$1" "$id-a")
desc_b=$(stage_akit "$2" "$id-b")
dir="$BENCH_DIR/ab/$id"
$BENCH_SSH "mkdir -p $dir && echo '$desc_a' > $dir/a.ref && echo '$desc_b' > $dir/b.ref && touch $dir/status && cat > $dir/params" <<EOP
scenarios=$scenarios protocols=$protocols rounds=$rounds duration=$duration warmup=$warmup args=$args fit=$fit_rev
EOP
$BENCH_SSH "cat > $dir/rounds.sh && chmod +x $dir/rounds.sh" <<EOP
#!/bin/sh
cd $dir
for r in \$(seq 1 $rounds); do
  for v in a b; do
    $fit_dir/bin/fitbench -root $fit_dir -bin $fit_dir/bin -akit-image akit-bench:$id-\$v \\
      -servers akit -idle "" -scenarios $scenarios -protocols $protocols \\
      -duration $duration -warmup $warmup $args -out $dir/\$v-\$r.json > \$v-\$r.log 2>&1
    echo "\$v-\$r exit \$?" >> status
  done
done
EOP
$BENCH_SSH "cat > $dir/run.sh && chmod +x $dir/run.sh" <<EOP
#!/bin/sh
cd $dir
$QUIET run ./rounds.sh > run.log 2>&1
touch done
docker rmi akit-bench:$id-a akit-bench:$id-b >/dev/null 2>&1 || true
EOP
$BENCH_SSH "cd $dir && setsid nohup ./run.sh > /dev/null 2>&1 < /dev/null &" < /dev/null
mkdir -p "$local_dir/$id"
echo "ab: started $id on the benchmark host (A: $desc_a; B: $desc_b); bench/ab.sh -report $id (or -wait $id)"
