# 部署 akit

[English](README.md) | [简体中文](README.zh-CN.md)

akit 和 fit 一样，是一个静态二进制加一个配置文件，打包方式与
[fit 的 deploy 目录](https://github.com/lesismal/fit/blob/main/deploy/README.zh-CN.md)打包 fit 的方式相同：发布包、
容器镜像、Kubernetes 的 Helm chart、systemd unit。CI 的 `packaging` 任务（[ci.yml](../.github/workflows/ci.yml)）
按这里写的步骤逐一构建并运行它们；[发布工作流](../.github/workflows/release.yml)在打标签时发布它们。背后的取舍见
[决策 009](../docs/decisions/009-packaging.zh-CN.md)。

## 发布包

打 `v<semver>` 标签会把 akit 的二进制附到该标签的 GitHub Release 上，每个与 LICENSE 和 README 一起打包为
`akit_<version>_<os>_<arch>.tar.gz`，另附 Helm chart 的包，SHA-256 校验和在 `checksums.txt` 里：

| 包 | 二进制 |
| --- | --- |
| `linux_amd64`、`linux_arm64` | 静态：Rust 的 musl 目标，musl 链接进二进制，运行时不加载任何库；可在 akio 能运行的任何 Linux 内核上运行（5.19 起可用 io_uring，否则 epoll） |
| `darwin_amd64`、`darwin_arm64` | macOS，与所有 macOS 程序一样链接系统库 |

```bash
make dist                       # 本机能构建的所有目标，放在 dist/ 并检查
make dist VERSION=v0.1.0 TARGETS=linux/amd64
make dist-check                 # 再次检查 dist/
```

[scripts/dist.sh](../scripts/dist.sh) 用 `cargo build --release --locked` 构建，musl 目标用 `rust-lld` 链接，
因此任何主机都能构建 Linux 包而无需交叉工具链（macOS 主机能构建全部四个）；`VERSION`（默认取 git describe）
是 `akit version` 打印的版本，也用于包名。[scripts/check-dist.sh](../scripts/check-dist.sh) 检查校验和、
每个包里的文件、每个二进制的平台、Linux 二进制是否静态，并运行本机能运行的那个。二进制保留符号，panic 的
backtrace 能给出函数名。

校验下载的文件：

```bash
sha256sum -c --ignore-missing checksums.txt
```

## 容器镜像

仓库根目录的 [Dockerfile](../Dockerfile) 构建静态 musl 二进制，放在
[distroless 的 static 镜像](https://github.com/GoogleContainerTools/distroless)上，以其 `nonroot` 用户（65532）
运行，与 fit 的镜像相同。镜像里只有 `/usr/local/bin/akit`，以及 `/etc/akit/akit.json` 处的一个占位配置，在
8080 端口回答所有请求。

```bash
docker build -t akit .
docker run --rm -p 8080:8080 -v "$PWD/hello.conf:/etc/akit/hello.conf:ro" akit run -c /etc/akit/hello.conf
```

- **自己的配置**放在 `/etc/akit/akit.json`，或放在任意位置并以 `run -c <路径>` 作为命令。akit 读取 fit 的配置：
  为 fit 镜像写的配置，改一下路径即可在这里运行。
- **1024 以下的端口**：Docker 允许容器的用户绑定；在别处用 sysctl `net.ipv4.ip_unprivileged_port_start=0`
  放开，Helm chart 就是这样做的。
- **状态**（如 ACME 的证书）放在 volume 里，在配置的存储目录中指定。
- **Admin API** 默认在容器内的 127.0.0.1:2019 监听。离开 loopback 需要 TLS，akit 尚不支持；用 `docker exec`
  访问，或给它一个放在 volume 里的 Unix socket（`unix//run/akit/admin.sock`）。
- **Docker provider** 读取属于 root 的 `/var/run/docker.sock`：以只读方式挂载并以 root 运行 akit
  （`--user 0`），或把 provider 的 `endpoint` 指向一个 socket 代理。
- **io_uring**：除非监听器要求 io_uring，akio 使用 epoll。Docker 默认的 seccomp 配置拒绝 io_uring 的系统调用，
  所以在容器里要求 io_uring 的监听器会回退到 epoll，并给出警告。
- **多平台**：`docker buildx build --platform linux/amd64,linux/arm64 .` 在构建机自己的平台上交叉编译每个目标，
  用 `rust-lld` 链接，不需要模拟。`--build-arg VERSION=v1.2.0` 设置 `akit version` 打印的版本。`make image`
  为本机平台构建。

打 `v<semver>` 标签会把 linux/amd64 和 linux/arm64 的镜像发布为 `ghcr.io/lesismal/akit:<version>` 和
`:<major>.<minor>`（预发布版本，如 `v1.2.0-rc.1`，只发布其版本号标签）。

## Helm chart

[helm/akit](helm/akit) 在集群里把 akit 作为 Gateway API 和 Ingress 的控制器运行，使用 Kubernetes provider
（I59、I60）：akit 服务其 GatewayClass `akit` 的 Gateway 及其路由并回写状态，也服务其 IngressClass `akit` 的
Ingress。它就是 fit 的 chart 换成 akit 的名字：控制器是 `lesismal.github.io/akit` 而不是 fit 的，所以 akit 和
fit 可以在同一个集群里运行，各自服务自己的类。

```bash
kubectl apply --server-side -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.3.0/standard-install.yaml
helm install akit deploy/helm/akit
```

先安装 Gateway API 的 CRD：只有集群里有这些 CRD 时 chart 才会创建 GatewayClass（也可以装好 CRD 后再 upgrade
release）。experimental 通道的 `experimental-install.yaml` 还会加上 TLSRoute、TCPRoute 和 UDPRoute，akit 同样
支持。

chart 创建的内容：

- akit 的 **Deployment**：以镜像的用户运行，根文件系统只读，没有 capability，并设置允许它绑定 Gateway 所用端口的
  sysctl。其配置放在 ConfigMap 里：一个 akit 自己的监听器，在 `healthPort`（9099）上回答 kubelet 对 `/healthz`
  的探测，以及 Kubernetes provider，由它加上每个 Gateway 的监听器和路由。配置变化会滚动更新 pod。HTTPS 监听器的
  证书写到 `/tmp`，一个内存中的 `emptyDir`。
- 前面的 **Service**，带 Gateway 和 Ingress 监听的端口，默认 80 和 443（`service.ports`；用
  `service.type: LoadBalancer` 对外暴露）。
- 一个 **ClusterRole**：在整个集群范围 list 和 watch namespace、Service、EndpointSlice、Secret（只列出
  `kubernetes.io/tls` 类型的）、Gateway API 的各种对象，以及在 `ingress.enabled` 时的 Ingress 和 IngressClass；
  并 patch GatewayClass、Gateway 和路由的 status。
- **GatewayClass** `akit`，以及在 `ingress.enabled`（默认开启）时的 **IngressClass** `akit`
  （`ingress.default: true` 使其成为集群的默认类）。

| 值 | 默认 | 作用 |
| --- | --- | --- |
| `image.repository`、`image.tag` | `ghcr.io/lesismal/akit`、chart 的 appVersion | 镜像。 |
| `replicas` | 1 | 每个副本都服务所有 Gateway 并写入相同的 status。 |
| `controllerName` | `lesismal.github.io/akit` | 这些类的控制器。 |
| `gatewayClass.create`、`gatewayClass.name` | `true`、`akit` | GatewayClass。 |
| `ingress.enabled`、`ingress.className`、`ingress.default` | `true`、`akit`、`false` | Ingress 及其类。 |
| `ingress.httpPort`、`ingress.httpsPort` | 80、443 | 服务 Ingress 的端口。 |
| `provider` | `{}` | provider 的更多设置：`namespaces`、`backends`、`addresses`、`max_objects`、`request_timeout`、`listen_host`、`certificates_dir`。 |
| `healthPort` | 9099 | 探测用的端口，与 Gateway 使用的端口分开。 |
| `config` | `{}` | 用一份完整配置代替 chart 生成的；应保留其健康检查监听器、路由和 provider。 |
| `extraArgs` | `[]` | `akit run` 的更多参数。 |
| `service.*` | ClusterIP，端口 80 和 443 | Service。 |

其余的值（`resources`、`nodeSelector`、`tolerations`、`affinity`、`podSecurityContext`、`securityContext`、
`podAnnotations`、`serviceAccount`）与多数 chart 相同；见 [values.yaml](helm/akit/values.yaml)。chart 的
`appVersion` 就是发布的版本：与标签不一致时发布工作流会拒绝。

在发布镜像之前，先构建镜像并载入集群（例如 `kind load docker-image akit:local`），然后用
`--set image.repository=akit --set image.tag=local` 安装。

## systemd

[systemd](systemd) 里有 `akit.service`，用于配置在 `/etc/akit/akit.json` 的 akit；还有 `akit@.service`，用于
一台主机上运行多个 akit，`akit@<name>` 运行 `/etc/akit/<name>.json`。

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

- akit 以系统用户 `akit` 运行，只有 `CAP_NET_BIND_SERVICE`，因此可以绑定 80 和 443 端口；文件系统只读，除了
  `/var/lib/akit`（`StateDirectory`；把 ACME 的存储指向这里）和 `/run/akit`（`RuntimeDirectory`；用于 Admin API
  的 Unix socket，`unix//run/akit/admin.sock`）。`akit@<name>` 使用 `/var/lib/akit/<name>` 和
  `/run/akit/<name>`。
- 启动前用 `akit validate` 检查配置。`systemctl reload akit` 检查新配置并向 akit 发送 SIGHUP：配置无效时 reload
  失败，akit 保留正在运行的配置。
- `systemctl stop akit` 发送 SIGTERM；akit 最多等待 30 秒（`-shutdown-timeout`）让进行中的请求完成，systemd
  最多等待 akit 40 秒。
- Docker provider 需要 socket 所属的组：`systemctl edit akit`，写入 `[Service]` 和
  `SupplementaryGroups=docker`。其他修改同样放在 drop-in 里。

[systemd/test.sh](systemd/test.sh) 在一台用完即弃的机器上按上面的步骤安装这些 unit 并检查它们（CI 以 root 运行）：
akit 以用户 `akit` 服务 80 端口，`systemctl reload` 时接受修改后的配置，拒绝无效配置并继续服务，`akit@.service`
能运行第二个实例。

## 发布需要维护者做的事

工作流用仓库自己的 `GITHUB_TOKEN` 发布（Release 需要 `contents: write`，ghcr.io 需要 `packages: write`），
不需要其他配置。第一次打标签之前：

- ghcr.io 上的包 `akit` 在第一次推送时创建；如果希望不登录就能拉取镜像，在其设置里设为公开。
- 在标签指向的提交里，把 [Chart.yaml](helm/akit/Chart.yaml) 的 `version` 和 `appVersion` 改成标签的版本。
- 目前没有任何签名：镜像没有 cosign 签名或 SLSA 来源证明，发布包除了 `checksums.txt` 之外没有签名，macOS
  二进制没有公证（浏览器下载的二进制运行前 Gatekeeper 会询问；`xattr -d com.apple.quarantine akit` 后即可运行）。
  这些都需要维护者的密钥或身份。
- chart 附在 Release 上，没有发布到 chart 仓库或 OCI registry。
