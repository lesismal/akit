#!/bin/sh
# test.sh runs this example as README.md says and checks it, as fit's example's test.sh does
# (M4's exit criterion): akit answers for whoami.localhost from both replicas, follows a third
# when the app is scaled up, drops the route when they stop, and takes it back when they start
# again. CI runs it; it needs Docker Compose and curl and port $AKIT_PORT (8080) free. With
# AKIT_IMAGE set it runs that image instead of building this checkout's.
set -eu
cd "$(dirname "$0")"
port=${AKIT_PORT:-8080}
seen=$(mktemp)
trap 'docker compose logs akit | tail -50; docker compose down --remove-orphans; rm -f "$seen"' EXIT

if [ -n "${AKIT_IMAGE:-}" ]; then
	docker compose up -d --no-build
else
	docker compose up -d --build
fi

# get prints the body and, last, the status of a request for whoami.localhost.
get() { curl -s -m 2 -H 'Host: whoami.localhost' -w '\n%{http_code}' "http://127.0.0.1:$port/" || true; }

# wait_for waits up to 30s for get's last line, the status, to be $1.
wait_for() {
	for _ in $(seq 60); do
		[ "$(get | tail -n 1)" = "$1" ] && return 0
		sleep 0.5
	done
	echo "whoami.localhost never answered $1; last: $(get)" >&2
	return 1
}

# replicas waits up to 30s for $1 replicas to have answered, each as it passes its first
# health check.
replicas() {
	: > "$seen"
	for _ in $(seq 60); do
		get | sed -n 's/^Hostname: //p' >> "$seen"
		[ "$(sort -u "$seen" | wc -l)" -ge "$1" ] && return 0
		sleep 0.5
	done
	echo "answered by $(sort -u "$seen" | wc -l) replicas, not $1" >&2
	return 1
}

wait_for 200
replicas 2
echo "two replicas answer"

# Another host has no route: the config's own 404.
[ "$(curl -s -m 2 -o /dev/null -w '%{http_code}' -H 'Host: other.localhost' "http://127.0.0.1:$port/")" = 404 ]

docker compose up -d --no-build --scale whoami=3 whoami
replicas 3
echo "a third replica answers once started"

docker compose stop whoami
wait_for 404
echo "the route goes when the app stops"
docker compose start whoami
wait_for 200
echo "and comes back when it starts"
echo "compose example: ok"
