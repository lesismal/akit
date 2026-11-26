# 从 Traefik 迁移

[English](migrating-from-traefik.md) | [简体中文](migrating-from-traefik.zh-CN.md)

akit 从 Traefik 借鉴了 provider（把 Docker 标签、Consul 目录、KV 存储和 Kubernetes 资源变成路由）和中间件；它另外提供静态文件
服务、缓存，每个连接的开销也更小。没有转换器；本指南把 Traefik 的各部分对应到 akit 的配置，[配置参考](../reference/config.zh-CN.md)
列出每个字段。

## 各部分的对应

| Traefik | akit |
| --- | --- |
| entryPoints | 监听器：`listener web { address :80; protocols http/1.1 h2c }` |
| entryPoint 的 `http.tls` | 监听器的 `tls` |
| HTTP router | 路由，或一组主机名的 `site` 中的路由；`entryPoints` 即路由的 `listeners` |
| router 的 `rule` | 路由的 `match.expr`，或 `match` 的字段 |
| router 的 `priority` | 路由的 `priority` |
| service、`loadBalancer.servers` | 上游及其 `addresses`，配合 `proxy` 处理器 |
| `loadBalancer.healthCheck` | 上游的 `health_check` |
| `loadBalancer.sticky.cookie` | 上游的 `sticky_cookie` |
| 加权 service | proxy 的 `upstreams`，按 `weight` |
| 镜像 service | proxy 的 `mirror` |
| 故障转移 service | proxy 的 `failover` |
| serversTransport（`rootCAs`、`serverName`、客户端证书） | 上游的 `protocol` `h2` 以及 `tls_ca_file`、`tls_server_name`、`tls_cert_file` |
| 带 `HostSNI` 的 TCP router | `tcp` 监听器和带 `server_names` 的服务，透传 TLS |
| UDP router | `udp` 监听器和服务 |
| certificatesResolvers（ACME） | `acme`，支持 HTTP-01 和 TLS-ALPN-01，监听器的 `tls.acme` 列出名称；带主机名的路由默认 HTTPS |
| `tls.certificates` | 监听器的 `tls.certificates` |
| 带 `clientAuth` 的 `tls.options` | 监听器的 `tls.client_auth` |
| `accessLog` | `access_log` |
| `metrics.prometheus` | `metrics`，在 Admin API 的 `/metrics` |
| `tracing`（OpenTelemetry） | `telemetry` |
| `api.dashboard` | Admin API 的只读仪表盘 `/dashboard/` |

## 规则

`match.expr` 接受 Traefik 语法中双方共有的匹配器，包括反引号：

```fit
listener web :8080

upstream app 127.0.0.1:9000

route api {
	match {
		expr "Host(`example.com`) && (PathPrefix(`/api`) || Method(`POST`))"
	}
	handler proxy { upstream app }
}
```

它的匹配器有 `Host`、`Path`、`PathPrefix`、`PathRegexp`、`Method`、`Header(name, value...)`、`Query(key, value...)` 和 `RemoteIP`
（Traefik 的 `ClientIP`）。没有 `HostRegexp` 和 `HeaderRegexp`：以 `*.`（一级）或 `**.`（一级或多级）开头的主机名能满足大多数需要。

## 中间件

| Traefik | akit |
| --- | --- |
| `StripPrefix`、`AddPrefix`、`ReplacePathRegex` | `rewrite`：`strip_prefix`、`add_prefix`、`regexp` 和 `replacement` |
| `RedirectScheme`、`RedirectRegex` | `redirect` |
| `Headers` | `headers` |
| `BasicAuth` | `basic_auth` |
| `ForwardAuth` | `forward_auth` |
| `RateLimit` | `rate_limit` |
| `InFlightReq` | `concurrency_limit` |
| 带 `maxRequestBodyBytes` 的 `Buffering` | `body_limit` |
| `CircuitBreaker` | `circuit_breaker` |
| `Compress` | `compress` |
| `Retry` | proxy 的 `retry` |
| `PassTLSClientCert` | `client_cert` |
| `IPAllowList` | 路由的 `match.remote_ip`，再加一条答 403 的路由 |
| 插件 | WebAssembly 插件（`wasm`），或以 `akit build` 编入的模块 |

## Provider

| Traefik | akit 的 `providers` |
| --- | --- |
| file provider | 配置文件本身，以 `akit run -watch` 在变化时重新应用 |
| Docker | `docker`，标签以 `akit.` 开头（见下） |
| Consul Catalog | `consul`，来自服务的标签 |
| Consul KV、etcd | `consul_kv`、`etcd`：键中保存监听器、路由、上游和服务的 JSON 对象 |
| Kubernetes Gateway API | `kubernetes`，提供其 GatewayClass 下的 Gateway 及其 HTTPRoute、GRPCRoute、TLSRoute、TCPRoute 和 UDPRoute |
| Kubernetes Ingress | `kubernetes` 的 `ingress`，用于 akit 的 IngressClass |
| Kubernetes `allowExternalNameServices` | 尚不支持 |
| Kubernetes CRD（IngressRoute） | 没有：请使用 Gateway API |

配置文件中写的内容优先于所有 provider，排在前面的 provider 优先于后面的；Admin API 的 `/config/effective` 给出合并的结果及每一部分
的来源。

```fit
listener web :80

provider { docker { endpoint unix:///var/run/docker.sock } }
```

Docker 标签与 Traefik 的相似，以 `akit.`（`label_prefix` 可以修改）而不是 `traefik.` 开头：

| Traefik | akit |
| --- | --- |
| `traefik.enable=true` | `akit.enable=true` |
| `traefik.http.routers.web.rule=Host(...)` | `akit.routes.web.host=a.example`、`akit.routes.web.path=/api/*`，或 `akit.routes.web.expr=Host(...)` |
| `traefik.http.services.app.loadbalancer.server.port=8080` | `akit.port=8080` |
| `traefik.http.routers.web.middlewares=...` | `akit.routes.web.middleware=[{"module": "compress"}]`，写成 JSON |
| `traefik.docker.network=net` | `akit.network=net` |

## 工作方式的不同

- **一个配置。** Traefik 把静态配置（entryPoints、provider）和动态配置（router、service、middleware）分开。akit 只有一个配置；其
  监听器可以在重载时改变，`providers`、`admin` 和 `acme` 在 akit 重启时生效。
- **路由按具体程度选择**：先看 `priority`，然后是最具体的主机和路径，而不是规则的长度。
- **没有的**：DNS 验证，因而无法从 ACME 获得通配符证书；Traefik 自己的 CRD；到上游的 HTTP/3。
