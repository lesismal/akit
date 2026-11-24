#!/bin/sh
# run.sh runs the Gateway API's conformance suite against akit, as fit's
# test/conformance/run.sh runs it against fit: it makes akit's GatewayClass in
# the cluster KUBECONFIG names, starts akit with the Kubernetes provider, whose
# Gateways listen on 127.0.0.1 and say so in their status, and runs the suite
# with the profile akit claims, GATEWAY-HTTP. The cluster's pods must be
# reachable from this machine, through a route to the kind node. See
# README.md.
#
# Usage: KUBECONFIG=<path> tests/conformance/run.sh <akit> <suite> [suite flags...]
#
# <akit> is akit's binary, and <suite> the test binary of fit's
# test/conformance module (go test -c), which needs nothing of fit's. Gateways
# listen on ports 80 and 443, so akit needs to bind them. akit's output goes to
# $AKIT_LOG (akit.log when unset), and the suite's report to $REPORT
# (report.yaml).
set -eu
: "${KUBECONFIG:?KUBECONFIG must name the cluster's kubeconfig}"
akit=$(realpath "$1")
suite=$(realpath "$2")
shift 2
kubeconfig=$(realpath "$KUBECONFIG")
class=akit
work=$(mktemp -d)

kubectl apply -f - <<END
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata: {name: $class}
spec: {controllerName: lesismal.github.io/akit}
END
cat > "$work/akit.json" <<END
{
  "listeners": [{"name": "base", "address": "127.0.0.1:0"}],
  "providers": [{"kubernetes": {"kubeconfig": "$kubeconfig", "controller_name": "lesismal.github.io/akit",
    "listen_host": "127.0.0.1", "addresses": ["127.0.0.1"]}}]
}
END
"$akit" run -c "$work/akit.json" > "${AKIT_LOG:-akit.log}" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true; wait $pid 2>/dev/null || true; rm -rf "$work"; kubectl delete gatewayclass "$class" --ignore-not-found >/dev/null 2>&1 || true' EXIT

version=$("$akit" version | awk '{print $2; exit}')
"$suite" -test.v -test.timeout 60m -gateway-class "$class" -conformance-profiles GATEWAY-HTTP \
	-organization lesismal -project akit -url https://github.com/lesismal/akit -version "$version" \
	-contact https://github.com/lesismal/akit/issues -report-output "${REPORT:-report.yaml}" "$@"
