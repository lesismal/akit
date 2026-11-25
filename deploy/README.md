# Deploying akit

[English](README.md) | [简体中文](README.zh-CN.md)

akit is one static binary with one config file, as fit is, and is packaged as
[fit's deploy directory](https://github.com/lesismal/fit/blob/main/deploy/README.md) packages fit: release
archives, a container image, a Helm chart for Kubernetes, and systemd units. CI's `packaging` job
([ci.yml](../.github/workflows/ci.yml)) builds each and runs it as written here; the
[release workflow](../.github/workflows/release.yml) publishes them for a tag. See
[decision 009](../docs/decisions/009-packaging.md) for the choices behind them.

## Release archives

A tag `v<semver>` attaches akit's binaries to its GitHub Release, each packed with the LICENSE and
the READMEs in `akit_<version>_<os>_<arch>.tar.gz`, with the Helm chart's package, and their
SHA-256 sums in `checksums.txt`:

| Archive | Binary |
| --- | --- |
| `linux_amd64`, `linux_arm64` | Static: Rust's musl targets, musl linked in, nothing loaded at run time; runs on any Linux kernel akio runs on (io_uring from 5.19, epoll otherwise) |
| `darwin_amd64`, `darwin_arm64` | macOS, linked against its system libraries as every macOS program is |

```bash
make dist                       # every target this host can build, in dist/, checked
make dist VERSION=v0.1.0 TARGETS=linux/amd64
make dist-check                 # checks dist/ again
```

[scripts/dist.sh](../scripts/dist.sh) builds with `cargo build --release --locked`, linking the
musl targets with `rust-lld`, so any host builds the Linux archives with no cross toolchain (a macOS
host builds all four); `VERSION` (git describe by default) is what `akit version` prints and names
the archives. [scripts/check-dist.sh](../scripts/check-dist.sh) checks the sums, each archive's
files, each binary's platform, that the Linux ones are static, and runs the one this host can run.
The binaries keep their symbols, so a panic's backtrace names its functions.

To check a download:

```bash
sha256sum -c --ignore-missing checksums.txt
```

## The container image

The [Dockerfile](../Dockerfile) at the repository's root builds the static musl binary and puts it
on [distroless's static image](https://github.com/GoogleContainerTools/distroless), run as its
`nonroot` user (65532), as fit's image is. The image holds only `/usr/local/bin/akit` and a
placeholder config at `/etc/akit/akit.json`, which answers every request on port 8080.

```bash
docker build -t akit .
docker run --rm -p 8080:8080 -v "$PWD/hello.conf:/etc/akit/hello.conf:ro" akit run -c /etc/akit/hello.conf
```

- **Your config** goes at `/etc/akit/akit.json`, or anywhere with `run -c <path>` as the command.
  akit reads fit's configs: a config written for fit's image runs here with its paths moved.
- **Ports below 1024**: Docker lets a container's user bind them; elsewhere, open them with the
  sysctl `net.ipv4.ip_unprivileged_port_start=0`, as the Helm chart does.
- **State** such as ACME's certificates goes in a volume, named in the config's storage directory.
- **The Admin API** listens on 127.0.0.1:2019 by default, inside the container. Off loopback it
  needs TLS, which akit does not support yet; reach it with `docker exec`, or give it a Unix socket
  (`unix//run/akit/admin.sock`) in a volume.
- **The Docker provider** reads `/var/run/docker.sock`, which is root's: mount it read-only and run
  akit as root (`--user 0`), or point the provider's `endpoint` at a socket proxy.
- **io_uring**: akio uses epoll unless a listener asks for io_uring. Docker's default seccomp
  profile refuses io_uring's system calls, so a listener that asks for it falls back to epoll in a
  container, with a warning.
- **Several platforms**: `docker buildx build --platform linux/amd64,linux/arm64 .` cross-compiles
  each on the builder's own platform, linked by `rust-lld`, without emulation.
  `--build-arg VERSION=v1.2.0` sets what `akit version` prints. `make image` builds for this
  machine.

A tag `v<semver>` publishes the image for linux/amd64 and linux/arm64 as
`ghcr.io/lesismal/akit:<version>` and `:<major>.<minor>` (a pre-release, `v1.2.0-rc.1`, only as
its version).

## The Helm chart

[helm/akit](helm/akit) runs akit in a cluster as a controller of the Gateway API and of Ingresses,
with the Kubernetes provider (I59, I60): akit serves the Gateways of its GatewayClass `akit` and
their routes, writing their status back, and the Ingresses of its IngressClass `akit`. It is
fit's chart with akit's names: its controller is `lesismal.github.io/akit`, not fit's, so akit and
fit may run in one cluster, each serving its own classes.

```bash
kubectl apply --server-side -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.3.0/standard-install.yaml
helm install akit deploy/helm/akit
```

Install the Gateway API's CRDs first: the chart makes the GatewayClass only when the cluster has
them (or upgrade the release after installing them). The experimental channel's
`experimental-install.yaml` adds TLSRoutes, TCPRoutes and UDPRoutes, which akit serves as well.

What it makes:

- A **Deployment** of akit, as the image's user, with a read-only root file system, no
  capabilities, and the sysctl that lets it bind the ports Gateways listen on. Its config, in a
  ConfigMap, is a listener of akit's own on `healthPort` (9099) answering the kubelet's probes on
  `/healthz`, and the Kubernetes provider, which adds each Gateway's listeners and routes. A
  changed config rolls the pods. The certificates of HTTPS listeners are written to `/tmp`, an
  `emptyDir` in memory.
- A **Service** in front of it with the ports the Gateways and Ingresses listen on, 80 and 443 by
  default (`service.ports`; `service.type: LoadBalancer` to expose them).
- A **ClusterRole** to list and watch, across the cluster, namespaces, Services, EndpointSlices,
  Secrets (only those of type `kubernetes.io/tls` are listed), the Gateway API's kinds and, with
  `ingress.enabled`, Ingresses and IngressClasses, and to patch the status of GatewayClasses,
  Gateways and routes.
- The **GatewayClass** `akit` and, with `ingress.enabled` (the default), the **IngressClass**
  `akit` (`ingress.default: true` makes it the cluster's default).

| Value | Default | What it sets |
| --- | --- | --- |
| `image.repository`, `image.tag` | `ghcr.io/lesismal/akit`, the chart's appVersion | The image. |
| `replicas` | 1 | Each replica serves every Gateway and writes the same status. |
| `controllerName` | `lesismal.github.io/akit` | The classes' controller. |
| `gatewayClass.create`, `gatewayClass.name` | `true`, `akit` | The GatewayClass. |
| `ingress.enabled`, `ingress.className`, `ingress.default` | `true`, `akit`, `false` | Ingresses and their class. |
| `ingress.httpPort`, `ingress.httpsPort` | 80, 443 | The ports Ingresses are served on. |
| `provider` | `{}` | More of the provider's settings: `namespaces`, `backends`, `addresses`, `max_objects`, `request_timeout`, `listen_host`, `certificates_dir`. |
| `healthPort` | 9099 | The port of the probes, apart from any a Gateway uses. |
| `config` | `{}` | A whole config in place of the chart's; keep its health listener, route and provider. |
| `extraArgs` | `[]` | More flags for `akit run`. |
| `service.*` | ClusterIP, ports 80 and 443 | The Service. |

The rest (`resources`, `nodeSelector`, `tolerations`, `affinity`, `podSecurityContext`,
`securityContext`, `podAnnotations`, `serviceAccount`) are as in most charts; see
[values.yaml](helm/akit/values.yaml). The chart's `appVersion` is the release's version: the
release workflow refuses a tag it does not match.

Until a release publishes the image, build it and load it into the cluster (`kind load
docker-image akit:local`, say) and install with `--set image.repository=akit --set
image.tag=local`.

## systemd

[systemd](systemd) has `akit.service`, for akit with its config at `/etc/akit/akit.json`, and
`akit@.service`, for several akits on one host, `akit@<name>` running `/etc/akit/<name>.json`.

```bash
make dist TARGETS=linux/amd64
tar -xzf dist/akit_*_linux_amd64.tar.gz
sudo install -m 0755 akit_*_linux_amd64/akit /usr/local/bin/akit
sudo install -m 0644 deploy/systemd/akit.service deploy/systemd/akit@.service /etc/systemd/system/
sudo install -m 0644 deploy/systemd/akit.sysusers /usr/lib/sysusers.d/akit.conf
sudo systemd-sysusers
sudo install -D -m 0644 deploy/docker/akit.json /etc/akit/akit.json
sudo systemctl daemon-reload
sudo systemctl enable --now akit
```

- akit runs as the system user `akit`, with `CAP_NET_BIND_SERVICE` alone, so it may bind ports 80
  and 443, and with the file system read-only but for `/var/lib/akit` (`StateDirectory`; point
  ACME's storage there) and `/run/akit` (`RuntimeDirectory`; for the Admin API's Unix socket,
  `unix//run/akit/admin.sock`). `akit@<name>` has `/var/lib/akit/<name>` and `/run/akit/<name>`.
- The config is checked with `akit validate` before akit starts. `systemctl reload akit` checks
  the new one and sends akit SIGHUP: a config that does not validate fails the reload, and akit
  keeps the one it runs.
- `systemctl stop akit` sends SIGTERM; akit waits up to 30s (`-shutdown-timeout`) for requests in
  flight, and systemd up to 40s for akit.
- The Docker provider needs the socket's group: `systemctl edit akit`, with `[Service]` and
  `SupplementaryGroups=docker`. Other changes go in drop-ins the same way.

[systemd/test.sh](systemd/test.sh) installs the units as above on a throwaway machine and checks
them (CI runs it as root): akit serves port 80 as the user `akit`, takes a changed config on
`systemctl reload`, refuses an invalid one there while it keeps serving, and `akit@.service` runs
a second instance.

## What a release needs from the maintainer

The workflow publishes with the repository's own `GITHUB_TOKEN` (`contents: write` for the
Release, `packages: write` for ghcr.io); nothing else is configured. Before the first tag:

- The ghcr.io package `akit` is made by the first push; make it public in its settings if the image
  should be pulled without a login.
- Bump `version` and `appVersion` in [Chart.yaml](helm/akit/Chart.yaml) to the tag's version in
  the commit the tag names.
- Nothing is signed yet: no cosign signature or SLSA provenance for the image, no signature for
  the archives beyond `checksums.txt`, and the macOS binaries are not notarized (Gatekeeper asks
  before running one downloaded by a browser; `xattr -d com.apple.quarantine akit` lets it run).
  Each needs a key or an identity of the maintainer's.
- The chart is attached to the Release; it is not published to a chart repository or an OCI
  registry.
