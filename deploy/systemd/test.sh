#!/bin/sh
# test.sh installs akit with these units as deploy/README.md says and checks
# them: akit serves port 80 as the user akit, takes a changed config on
# `systemctl reload`, refuses an invalid one there while it keeps serving,
# and akit@.service runs a second instance. CI runs it as root on a throwaway
# runner: it installs to /usr/local/bin, /etc and /usr/lib, and leaves the
# user akit behind.
#
# Usage: sudo deploy/systemd/test.sh <akit binary>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
install -m 0755 "$1" /usr/local/bin/akit
install -m 0644 "$here/akit.service" "$here/akit@.service" /etc/systemd/system/
install -m 0644 "$here/akit.sysusers" /usr/lib/sysusers.d/akit.conf
systemd-sysusers
mkdir -p /etc/akit
cleanup() {
	status=$?
	[ "$status" -eq 0 ] || journalctl -u akit -u akit@second --no-pager -n 50 || true
	systemctl stop akit akit@second || true
	rm -f /etc/systemd/system/akit.service /etc/systemd/system/akit@.service /etc/akit/akit.json /etc/akit/second.json
	systemctl daemon-reload
}
trap cleanup EXIT

# config writes a config to $1 serving body $3 on port $2.
config() {
	cat > "$1" <<END
{
  "listeners": [{"name": "web", "address": ":$2"}],
  "routes": [{"name": "hello", "handler": {"module": "respond", "params": {"body": "$3"}}}]
}
END
}

# expect waits up to 10s for port $1 to answer body $2.
expect() {
	for _ in $(seq 20); do
		[ "$(curl -s -m 2 "http://127.0.0.1:$1/")" = "$2" ] && return 0
		sleep 0.5
	done
	echo "port $1 never answered $2; last: $(curl -s -m 2 "http://127.0.0.1:$1/")" >&2
	return 1
}

systemd-analyze verify /etc/systemd/system/akit.service /etc/systemd/system/akit@second.service
config /etc/akit/akit.json 80 one
systemctl daemon-reload
systemctl start akit
expect 80 one
[ "$(ps -o user= -p "$(systemctl show -p MainPID --value akit)")" = akit ] || { echo "akit does not run as akit" >&2; exit 1; }

config /etc/akit/akit.json 80 two
systemctl reload akit
expect 80 two

echo '{"listeners": [{"name": "web", "address": "nope"}]}' > /etc/akit/akit.json
if systemctl reload akit 2>/dev/null; then echo "an invalid config reloaded" >&2; exit 1; fi
expect 80 two
systemctl is-active akit

config /etc/akit/second.json 8081 second
systemctl start akit@second
expect 8081 second
[ -d /var/lib/akit/second ] || { echo "no state directory for akit@second" >&2; exit 1; }
echo "systemd units: ok"
