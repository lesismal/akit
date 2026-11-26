#!/bin/sh
# modules.sh benchmarks each middleware akit has against fit's, on the benchmark host, without
# waiting for it (I56; docs/rules.md, rules 23-24; bench/README.md):
#
#   bench/modules.sh [options] [REF]   start a run of akit at REF (default: work, the working
#                                      tree) against fit at bench/FIT_REV; prints its id
#   bench/modules.sh -report ID        fetch it into bench/results/ID and summarize it
#   bench/modules.sh -wait ID          wait for it to finish, then -report it
#
# Options:
#
#   -rounds N       rounds, the servers' order turned each round        (default 3)
#   -duration S     seconds each module is loaded, after 2s of warm-up  (default 8)
#   -servers LIST   akit-epoll,akit-uring,fit                           (default: those three)
#   -modules LIST   the modules to load, as none,compress               (default: every one)
#
# Every server serves configs/modules/modules.json, each middleware on a route of its own
# (host <module>.bench) proxied to the backend, nginx answering 1 KiB of JSON
# (configs/modules/backend.conf); "none" is the proxy alone, "respond" a fixed answer. akit runs
# from its bench_serve example (crates/akit/examples/bench_serve.rs), the path `akit run` takes,
# on epoll and on io_uring; fit from its binary. Each runs natively, pinned to the CPUs fit's
# harness gives a server (0-6), wrk (7 threads, 256 connections) on 7-13 in fitbench's loadgen
# image, the backend on 14-15. For each module and server: requests a second, wrk's p99, and the
# server's CPU (its /proc/PID/stat) over the run. The run takes /root/bench.lock alone (quiet.sh).
set -eu
. "$(cd "$(dirname "$0")" && pwd)/lib.sh"

report() {
	id=$1
	out="$here/results/$id"
	mkdir -p "$out"
	$BENCH_SSH "cd $BENCH_DIR/runs/$id && tar cf - params ref status results.tsv run.log 2>/dev/null" |
		tar xf - -C "$out" 2>/dev/null || true
	if [ -e "$out/status" ]; then
		echo "bench: $id: $(cat "$out/status"); akit $(cat "$out/ref")"
	else
		echo "bench: $id: still running"
	fi
	[ -e "$out/results.tsv" ] && python3 "$here/modulesreport.py" "$out/results.tsv" | tee "$out/report.md"
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

rounds=3
duration=8
servers=akit-epoll,akit-uring,fit
only=all
while [ $# -gt 1 ] || { [ $# -eq 1 ] && case "$1" in -*) true ;; *) false ;; esac; }; do
	case "$1" in
	-rounds) rounds=$2 ;;
	-duration) duration=$2 ;;
	-servers) servers=$2 ;;
	-modules) only=$2 ;;
	*) die "unknown option $1" ;;
	esac
	shift 2
done
ref=${1:-work}

id=modules-$(date +%Y%m%d-%H%M%S)
dir="$BENCH_DIR/runs/$id"
stage_fit
src="$BENCH_DIR/src-$id"
$BENCH_SSH "rm -rf $src && mkdir -p $src $dir"
if [ "$ref" = work ]; then
	desc="work (uncommitted changes of $(git -C "$root" rev-parse --short HEAD))"
	(cd "$root" && COPYFILE_DISABLE=1 tar --no-xattrs -cf - --exclude ./target --exclude ./.git \
		--exclude ./.claude --exclude ./fuzz/target --exclude ./bench/results .) |
		$BENCH_SSH "tar xmf - -C $src"
else
	desc="$ref ($(git -C "$root" rev-parse --short "$ref"))"
	git -C "$root" archive --format=tar "$ref" | $BENCH_SSH "tar xmf - -C $src"
fi
echo "bench: building akit's bench_serve, $desc, on the host"
$BENCH_SSH "$host_path; cd $src && flock -s /root/bench.lock \
	cargo build -q --release -p akit --example bench_serve --target-dir $BENCH_DIR/target &&
	cp $BENCH_DIR/target/release/examples/bench_serve $dir/bench_serve &&
	cp $fit_dir/bin/fit $dir/fit &&
	cp bench/configs/modules/modules.json bench/configs/modules/backend.conf $dir/ &&
	gzip -dc crates/akit-plugin/tests/guests/headers-akit.wasm.gz > $dir/headers-akit.wasm &&
	gzip -dc crates/akit-plugin/tests/guests/headers-fit.wasm.gz > $dir/headers-fit.wasm &&
	printf 'a secret of the bench, 32 bytes or more' > $dir/secret" ||
	die "building akit $desc on the host failed"
$BENCH_SSH "rm -rf $src"
$BENCH_SSH "cd $dir && echo '$desc' > ref && echo 'rounds=$rounds duration=$duration servers=$servers modules=$only fit=$fit_rev' > params"

# The run itself, on the host.
$BENCH_SSH "cat > $dir/run.sh && chmod +x $dir/run.sh" <<'EOP'
#!/bin/bash
# Arguments: rounds duration servers modules (all, or a list).
set -u
cd "$(dirname "$0")"
rounds=$1 duration=$2 servers=${3//,/ } only=,$4,
SERVER_CPUS=0-6 CLIENT_CPUS=7-13 BACKEND_CPUS=14-15
api=$(python3 -c 'print("{\"items\": \"" + "x" * 1009 + "\"}", end="")')
sed "s|{{APIBODY}}|$api|" backend.conf > backend.run.conf
sed "s|{{BACKEND}}|127.0.0.1:19000|; s|{{DIR}}|$PWD|g" modules.json > modules.run.json
docker rm -f akit-modules-backend akit-modules-loadgen >/dev/null 2>&1
docker run -d --rm --name akit-modules-backend --network host --cpuset-cpus $BACKEND_CPUS \
	-v "$PWD/backend.run.conf:/etc/nginx/nginx.conf:ro" nginx:1.31.4 >/dev/null || exit 1
docker run -d --rm --name akit-modules-loadgen --network host --cpuset-cpus $CLIENT_CPUS \
	--ulimit nofile=1048576:1048576 --entrypoint sleep fitbench-loadgen:latest infinity >/dev/null || exit 1
trap 'docker rm -f akit-modules-backend akit-modules-loadgen >/dev/null 2>&1; [ -n "${pid:-}" ] && kill $pid 2>/dev/null' EXIT
ulimit -n 1048576
# module path headers...
modules=(
	"none /api/bench"
	"respond /api/bench"
	"rewrite /x/api/bench"
	"redirect /api/bench"
	"headers /api/bench"
	"rate_limit /api/bench"
	"concurrency_limit /api/bench"
	"body_limit /api/bench"
	"basic_auth /api/bench Authorization:_Basic_bWU6cHc="
	"client_cert /api/bench"
	"forward_auth /api/bench"
	"jwt /api/bench Authorization:_Bearer_eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtZSIsImV4cCI6NDEwMjQ0NDgwMH0.uUoQmkIKX2PFsRxppW-TjojteRvpAHvOxqo5Af-QK04"
	"circuit_breaker /api/bench"
	"compress /api/bench Accept-Encoding:_gzip"
	"compress_gzip /api/bench Accept-Encoding:_gzip"
	"cache /api/bench"
	"wasm /api/bench"
)
cpu() { awk '{print $14 + $15}' /proc/$1/stat; }
tick=$(getconf CLK_TCK)
echo -e "round\tserver\tmodule\trps\tp99_ms\tcpu_s\tseconds\tbad" > results.tsv
list=($servers)
for ((r = 1; r <= rounds; r++)); do
	n=${#list[@]}
	for ((k = 0; k < n; k++)); do
		s=${list[$(((k + r - 1) % n))]}
		# The wasm module's plugin, of each server's own ABI (decision 037).
		case $s in fit) cp -f headers-fit.wasm headers.wasm ;; *) cp -f headers-akit.wasm headers.wasm ;; esac
		case $s in
		akit-epoll) taskset -c $SERVER_CPUS ./bench_serve modules.run.json epoll 2>>server.log & ;;
		akit-uring) taskset -c $SERVER_CPUS ./bench_serve modules.run.json uring 2>>server.log & ;;
		fit) taskset -c $SERVER_CPUS ./fit run -c modules.run.json >>server.log 2>&1 & ;;
		esac
		pid=$!
		for i in $(seq 100); do
			curl -fsS -o /dev/null -H 'Host: respond.bench' http://127.0.0.1:18080/ 2>/dev/null && break
			sleep 0.1
		done
		for m in "${modules[@]}"; do
			set -- $m
			name=$1 path=$2
			shift 2
			[ "$only" = ,all, ] || [ "${only#*,$name,}" != "$only" ] || continue
			hdr=()
			for h in "$@"; do hdr+=(-H "${h//_/ }"); done
			host="${name//_/-}.bench"
			docker exec akit-modules-loadgen wrk -t 7 -c 256 -d 2s -H "Host: $host" "${hdr[@]}" \
				"http://127.0.0.1:18080$path" >/dev/null 2>&1
			c0=$(cpu $pid) t0=$(date +%s.%N)
			out=$(docker exec akit-modules-loadgen wrk -t 7 -c 256 -d ${duration}s --timeout 10s --latency \
				-H "Host: $host" "${hdr[@]}" "http://127.0.0.1:18080$path" 2>&1)
			c1=$(cpu $pid) t1=$(date +%s.%N)
			rps=$(echo "$out" | awk '/^Requests\/sec/ {print $2}')
			p99=$(echo "$out" | awk '$1 == "99%" {v = $2; if (v ~ /us$/) {sub(/us/, "", v); v /= 1000} else if (v ~ /ms$/) {sub(/ms/, "", v)} else if (v ~ /s$/) {sub(/s/, "", v); v *= 1000}; print v}')
			bad=$(echo "$out" | awk '/Non-2xx or 3xx/ {n += $5} /Socket errors/ {gsub(/,/, ""); n += $4 + $6 + $8 + $10} END {print n + 0}')
			cs=$(echo "($c1 - $c0) / $tick" | bc -l)
			secs=$(echo "$t1 - $t0" | bc -l)
			echo -e "$r\t$s\t$name\t${rps:-0}\t${p99:-0}\t$cs\t$secs\t$bad" >> results.tsv
			echo "round $r $s $name: ${rps:-0} req/s, p99 ${p99:-?} ms, bad $bad" >> run.log
		done
		kill $pid
		wait $pid 2>/dev/null
		pid=
		sleep 1
	done
done
EOP
$BENCH_SSH "cd $dir && setsid nohup sh -c '$QUIET run ./run.sh $rounds $duration $servers $only >> run.log 2>&1; echo \"exit \$?\" > status; rm -f bench_serve fit headers.wasm headers-akit.wasm headers-fit.wasm' > /dev/null 2>&1 < /dev/null &" < /dev/null
echo "bench: started $id on the benchmark host; bench/modules.sh -report $id (or -wait $id)"
