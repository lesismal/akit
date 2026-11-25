#!/bin/sh
# The stampede check, on the benchmark host (bench/stampede.sh starts it under quiet.sh): many
# clients ask a cache for one cold key at once, while the origin takes DELAY seconds to answer;
# the origin counts what reaches it. Run in this directory, with:
#
#   run.sh FIT_BIN AKIT_IMAGE CLIENTS KEYS DELAY
#
# Each server (nginx with proxy_cache_lock on, fit, akit, and akit with disable_lock as the
# control) runs alone in a container on the server's CPUs (0-6), host networking; h2load runs in
# fitbench's load generator image on 7-13; the origin, origin.py, on 14-15. For each protocol,
# KEYS cold keys, each asked for by CLIENTS clients at once: over HTTP/1.1 CLIENTS connections of
# one request each, over h2c CLIENTS streams on CLIENTS/100 connections. Prints one line a key:
# server, protocol, key, requests answered 200, fetches the origin counted.
set -u
fit_bin=$1
akit_image=$2
clients=$3
keys=$4
delay=$5
here=$(pwd)
loadgen=fitbench-loadgen
nginx_image=nginx:1.31.4
name=akit-stampede

cleanup() {
	docker rm -f $name $name-client >/dev/null 2>&1
	[ -n "${origin_pid:-}" ] && kill "$origin_pid" 2>/dev/null
	rm -rf "$here/nginx-cache"
}
trap cleanup EXIT INT TERM

taskset -c 14-15 python3 -I "$here/origin.py" 19103 "$delay" &
origin_pid=$!
docker run -d --name $name-client --network host --cpuset-cpus 7-13 --entrypoint sleep $loadgen infinity >/dev/null
for i in $(seq 50); do
	docker exec $name-client curl -sf -o /dev/null http://127.0.0.1:19103/ready && break
	sleep 0.1
done

start() {
	server=$1
	common="-d --name $name --network host --cpuset-cpus 0-6 --ulimit nofile=1048576:1048576"
	case $server in
	nginx)
		mkdir -p "$here/nginx-cache" && chmod 777 "$here/nginx-cache"
		docker run $common -v "$here/nginx.conf:/etc/nginx/nginx.conf:ro" \
			-v "$here/nginx-routes.conf:/etc/nginx/stampede-routes.conf:ro" \
			-v "$here/nginx-cache:/var/cache/nginx/stampede" $nginx_image >/dev/null
		;;
	fit)
		docker run $common -v "$fit_bin:/usr/local/bin/fit:ro" -v "$here/fit.conf:/etc/fit/fit.conf:ro" \
			ubuntu:24.04 /usr/local/bin/fit run -c /etc/fit/fit.conf >/dev/null
		;;
	akit | akit-nolock)
		docker run $common -v "$here/$server.json:/etc/akit/akit.json:ro" \
			"$akit_image" /usr/local/bin/akit run -c /etc/akit/akit.json >/dev/null
		;;
	esac
	for i in $(seq 100); do
		docker exec $name-client curl -sf -o /dev/null http://127.0.0.1:19180/ready &&
			docker exec $name-client curl -sf --http2-prior-knowledge -o /dev/null http://127.0.0.1:19181/ready &&
			return 0
		sleep 0.1
	done
	echo "$server did not start" >&2
	docker logs $name >&2
	return 1
}

for server in nginx fit akit akit-nolock; do
	docker rm -f $name >/dev/null 2>&1
	start $server || continue
	for proto in http/1.1 h2c; do
		for k in $(seq "$keys"); do
			path=/stampede/$server-$(echo $proto | tr -d /.)-$k
			if [ $proto = http/1.1 ]; then
				out=$(docker exec $name-client h2load --h1 -n "$clients" -c "$clients" -t 7 \
					"http://127.0.0.1:19180$path" 2>&1)
			else
				conns=$((clients / 100))
				out=$(docker exec $name-client h2load -n "$clients" -c "$conns" -m 100 -t 7 \
					"http://127.0.0.1:19181$path" 2>&1)
			fi
			ok=$(echo "$out" | sed -n 's/^status codes: \([0-9]*\) 2xx.*/\1/p')
			fetches=$(docker exec $name-client curl -sf "http://127.0.0.1:19103/count$path")
			echo "$server $proto $k ${ok:-?} ${fetches:-?}"
		done
	done
	docker rm -f $name >/dev/null 2>&1
done
