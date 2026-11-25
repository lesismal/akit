# The Gateway API's conformance suite

[English](README.md) | [简体中文](README.zh-CN.md)

M4's exit criterion (I60's and I66's "done when") is that akit passes the Gateway API conformance
profile fit claims, `GATEWAY-HTTP`, at the Gateway API version fit pins (v1.3.0). The
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

## The result

akit passes it: [reports/gateway-http-v1.3.0-2026-11-25.yaml](reports/gateway-http-v1.3.0-2026-11-25.yaml),
the report the suite wrote (I66, on the benchmark host, Linux 7.0, kind v0.30.0, akit at
`346ad8b`): **`GATEWAY-HTTP` core, 33 passed, 0 failed, 0 skipped**, the profile's core as fit
claims it (fit's I61: Gateway, HTTPRoute and ReferenceGrant, the same 33 tests). The suite's other
38 tests, of extended features and other profiles, are skipped because neither fit nor akit claims
them; a feature is claimed with a flag in `run.sh` once its tests pass (fit's decision 012).

- akit runs outside the cluster on akio's epoll backend: `akit run` has no switch for io_uring
  (akio's `Backend` is chosen per listener in code, and only akit's tests set it, through
  `AKIO_TEST_BACKEND`), so the suite cannot run against akit on io_uring. The provider's and the
  listeners' own tests run on both.
- The extended features have not been run against akit yet; the
  [M4 report](../../bench/results/m4-report-2026-11-25.md) says why and what is next.

CI runs it in the job "Gateway API conformance (kind)" ([ci.yml](../../.github/workflows/ci.yml)),
as fit's CI does: fit's suite module checked out at the commit akit reads, a kind cluster, the
experimental CRDs, akit built with `CAP_NET_BIND_SERVICE`, `run.sh`, and the report and akit's log
kept as an artifact.

## Running it

CI runs it on every push; elsewhere, a Linux machine with Docker runs it as below. On the
benchmark host, one session at a time (rules 23 and 30), under the shared lock.

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

   On a host that has another kind cluster, give this one pod and service subnets of its own
   (a kind config with `networking.podSubnet` and `serviceSubnet`, say `10.250.0.0/16` and
   `10.251.0.0/16`), so that the route to its pods does not clash with the other's.

4. Clean up (rule 29): `kind delete cluster --name akit`, the route
   (`sudo ip route del "$cidr"`), the suite's binary, the report and the log once read.

A failure the suite reports is fixed in the provider (`crates/akit-provider/src/kubernetes`),
with a test of its own beside fit's ported ones, and the suite run again.
