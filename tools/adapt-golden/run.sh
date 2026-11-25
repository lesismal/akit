#!/bin/sh
# The golden files of `akit adapt` and `akit fmt` (I73), written by fit itself: builds
# tools/adapt-golden/main.go against fit (beside akit or at FIT_DIR, offline from Go's module
# cache) and runs it from akit's root, with fit's examples' secrets set to the stand-ins the
# compatibility gate uses. Rerun when the fit pin moves (tests/compat/fixtures/PINS), and copy
# fit's cli/testdata/*.json and *.yaml to crates/akit/tests/testdata/adapt/fit first.
#
#   tools/adapt-golden/run.sh
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
fit=$(cd "${FIT_DIR:-$root/../fit}" && pwd)
out=$root/target/adapt-golden
mkdir -p "$out/src"
cp "$root/tools/adapt-golden/main.go" "$fit/go.sum" "$out/src/"
cat >"$out/src/go.mod" <<MOD
module adaptgolden

go 1.24

require github.com/lesismal/fit v0.0.0

replace github.com/lesismal/fit => $fit
MOD
(cd "$out/src" && GOFLAGS=-mod=mod GOPROXY=${GOPROXY:-off} go build -o "$out/fit-adapt" .)
for v in ES_PASSWORD KAFKA_PASSWORD MYSQL_PASSWORD PG_PASSWORD RABBITMQ_PASSWORD NATS_PASSWORD \
	PULSAR_TOKEN MQTT_PASSWORD INFLUX_TOKEN TD_PASSWORD MSSQL_PASSWORD DORIS_PASSWORD \
	OPENAI_API_KEY QDRANT_API_KEY MILVUS_TOKEN; do
	export "$v=example-password"
done
cd "$root" && "$out/fit-adapt"
rm -rf "$out"
