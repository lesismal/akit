#!/bin/sh
# run.sh runs fit's benchmark harness on the benchmark host with akit among the servers under
# test, without waiting for it (docs/rules.md, rules 23-24; bench/README.md):
#
#   bench/run.sh [options] [REF]   start a run of akit at REF (default: work, the working tree);
#                                  prints its id and returns
#   bench/run.sh -report ID        fetch it into bench/results/ID and summarize it
#   bench/run.sh -wait ID          wait for it to finish, then -report it
#
# Options, passed to fitbench:
#
#   -servers LIST     nginx,caddy,traefik,fit,akit       (default: those five)
#   -scenarios LIST   fixed,static,proxy                 (default: those three)
#   -protocols LIST   http/1.1,h2c,https/1.1,h2          (default: what akit serves so far)
#   -idle LIST        http1,tls                          (default: what akit holds so far)
#   -args FLAGS       more fitbench flags, as '-duration 15s -warmup 5s'
#
# The run waits on the host for /root/bench.lock alone (quiet.sh), so it measures a quiet host.
set -eu
. "$(cd "$(dirname "$0")" && pwd)/lib.sh"

report() {
	id=$1
	out="$here/results/$id"
	mkdir -p "$out"
	$BENCH_SSH "cd $BENCH_DIR/runs/$id && tar cf - params ref status results.json report.md run.log 2>/dev/null" |
		tar xf - -C "$out" 2>/dev/null || true
	if [ -e "$out/status" ]; then
		echo "bench: $id: $(cat "$out/status"); akit $(cat "$out/ref")"
	else
		echo "bench: $id: still running"
	fi
	[ -e "$out/report.md" ] && cat "$out/report.md"
	return 0
}

case "${1:-}" in
-report)
	report "$2"
	exit 0
	;;
-wait)
	until $BENCH_SSH "test -e $BENCH_DIR/runs/$2/status"; do sleep 30; done
	report "$2"
	exit 0
	;;
esac

servers=nginx,caddy,traefik,fit,akit
scenarios=fixed,static,proxy
protocols=http/1.1,h2c,https/1.1,h2
idle=http1,tls
args=
while [ $# -gt 1 ] || { [ $# -eq 1 ] && case "$1" in -*) true ;; *) false ;; esac; }; do
	case "$1" in
	-servers) servers=$2 ;;
	-scenarios) scenarios=$2 ;;
	-protocols) protocols=$2 ;;
	-idle) idle=$2 ;;
	-args) args=$2 ;;
	*) die "unknown option $1" ;;
	esac
	shift 2
done
ref=${1:-work}

id=run-$(date +%Y%m%d-%H%M%S)
stage_fit
desc=$(stage_akit "$ref" "$id")
dir="$BENCH_DIR/runs/$id"
$BENCH_SSH "mkdir -p $dir && cat > $dir/params && echo '$desc' > $dir/ref" <<EOP
servers=$servers scenarios=$scenarios protocols=$protocols idle=$idle args=$args fit=$fit_rev
EOP
$BENCH_SSH "cat > $dir/run.sh && chmod +x $dir/run.sh" <<EOP
#!/bin/sh
cd $dir
$QUIET run $fit_dir/bin/fitbench -root $fit_dir -bin $fit_dir/bin -akit-image akit-bench:$id \\
  -servers $servers -scenarios $scenarios -protocols $protocols -idle "$idle" $args \\
  -label $id -out $dir/results.json > run.log 2>&1
code=\$?
[ -e results.json ] && $fit_dir/bin/benchreport -in results.json > report.md 2>&1
echo "exit \$code" > status
docker rmi akit-bench:$id >/dev/null 2>&1 || true
EOP
$BENCH_SSH "cd $dir && setsid nohup ./run.sh > /dev/null 2>&1 < /dev/null &" < /dev/null
echo "bench: started $id on the benchmark host; bench/run.sh -report $id (or -wait $id)"
