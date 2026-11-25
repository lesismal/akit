# The Gateway API's conformance suite

[English](README.md) | [简体中文](README.zh-CN.md)

I60's "done when" is that akit passes the Gateway API's `GATEWAY-HTTP` conformance profile. The
suite is the Go package `sigs.k8s.io/gateway-api/conformance`: a test that applies its manifests
to a real cluster, waits for the implementation to accept and program them, and sends requests to
the Gateways' addresses. akit runs it the way fit does
([fit's decision 012](https://github.com/lesismal/fit/blob/main/docs/decisions/012-gateway-conformance.md)),
with fit's own suite module, which needs nothing of fit's: no Go code enters akit's tree.

[`run.sh`](run.sh) is fit's `test/conformance/run.sh` for akit: it makes akit's GatewayClass
(`akit`, controller `lesismal.github.io/akit`, so that it does not take fit's Gateways in a
cluster both serve), starts `akit run` outside the cluster with the Kubernetes provider (Gateways
bound on 127.0.0.1, that address written into their status by `addresses`), and runs the suite
with the profile akit claims.

## Running it

The maintainer runs it, on the benchmark host or another Linux machine with Docker, one session
at a time (rules 23 and 30); feature tasks do not.

1. A kind cluster, the Gateway API's CRDs at the version fit's module requires (v1.3.0, the
   experimental channel for TLSRoutes, TCPRoutes and UDPRoutes), and a route to the pod network
   through the node:

   ```sh
   kind create cluster --name akit --wait 120s --kubeconfig "$PWD/kubeconfig"
   export KUBECONFIG=$PWD/kubeconfig
   kubectl apply --server-side -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.3.0/experimental-install.yaml
   node=$(docker inspect -f '{{.NetworkSettings.Networks.kind.IPAddress}}' akit-control-plane)
   cidr=$(kubectl get node akit-control-plane -o jsonpath='{.spec.podCIDR}')
   sudo ip route add "$cidr" via "$node"
   ```

2. akit's binary, allowed to bind ports 80 and 443, and the suite's test binary from fit's
   module:

   ```sh
   cargo build --release -p akit
   sudo setcap cap_net_bind_service=+ep target/release/akit
   (cd ../fit/test/conformance && go test -c -o "$PWD/conformance.test" .)
   ```

3. The suite, its report in `report.yaml` and akit's log in `akit.log`:

   ```sh
   tests/conformance/run.sh target/release/akit ../fit/test/conformance/conformance.test
   ```

4. Clean up (rule 29): `kind delete cluster --name akit`, the route
   (`sudo ip route del "$cidr"`), the suite's binary, the report and the log once read.

A failure the suite reports is fixed in the provider (`crates/akit-provider/src/kubernetes`),
with a test of its own beside fit's ported ones, and the suite run again.
