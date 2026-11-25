# 反向代理与负载均衡

[English](reverse-proxy.md) | [简体中文](reverse-proxy.zh-CN.md)

akit 的 `proxy` 处理器把路由的请求发给上游，即一组有名字的后端，并提供负载均衡、健康检查、超时和重试。请求经 akio 的异步 HTTP
客户端发出，请求体和响应体双向流式转发，每一侧的背压都会暂停另一侧：慢客户端或慢后端只让 akit 为它保留有界的字节，从不占用
线程。[配置参考](../reference/config.zh-CN.md#上游)列出每个字段。

## 一个上游和一条路由

```fit
listener web :8080 http/1.1 h2c

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080 10.0.0.3:8080
	load_balancing least_conn
}

route api {
	match { path /api/* }
	handler proxy { upstream app }
}
```

代理发送的内容与 fit 相同：`X-Forwarded-For`、`X-Forwarded-Proto` 和 `X-Forwarded-Host`（来自客户端，或保留 `trusted_proxies`
中列出的代理所给的值），以及客户端的 `Host`，除非 `host_header` 指定了别的。客户端离开时，发往后端的请求随即放弃。

## 负载均衡

`load_balancing` 为每个请求选择一个地址：`round_robin`（默认），以及带 `weights` 的 `weighted_round_robin`；`least_conn`；
`random`；`p2c`，随机选两个取较空闲者；以及一致性哈希 `ip_hash`、`uri_hash`、`header_hash` 和 `cookie_hash`，在地址可用时让
同一客户端、路径、头部或 cookie 固定到同一地址。`sticky_cookie` 用 cookie 让每个客户端固定在它第一次去的地址上。

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	weights 3 1
	load_balancing weighted_round_robin
	sticky_cookie { name app_backend; max_age 1h }
}

route { handler proxy { upstream app } }
```

## 健康检查

被动地，在 `fail_timeout` 内请求失败 `max_fails` 次的地址被摘除 `fail_timeout`。主动地，`health_check` 按各自的周期以 HTTP 或
TCP 探测每个地址，连续 `fails` 次探测失败后摘除，连续 `passes` 次成功后恢复。没有可用地址的上游答 502；这时 proxy 的
`failover` 指定的上游接手。

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	max_fails 3
	fail_timeout 30s
	health_check {
		path /healthz
		interval 5s
		timeout 2s
		fails 2
		passes 2
	}
}

upstream maintenance 10.0.0.9:8080

route {
	handler proxy {
		upstream app
		failover maintenance
	}
}
```

## 超时、上限和重试

与后端的每次交换都有界：`connect_timeout`、`response_header_timeout`（到响应开始）、`response_idle_timeout`（响应体中的静默）
和 `response_timeout`（整个响应），以及路由的 `timeout`，涵盖所有尝试。超时的请求答 504。每个地址至多 `max_conns` 个连接，
至多 `max_pending` 个请求等待连接；超出时请求立即答 503，而不是无界地排队。

`retry` 把失败的请求发给另一个地址重试：无法连接的请求，以及（设置 `statuses` 时）得到这些状态码的请求。只有请求体完整到达的
请求才会重试，重试受预算（`budget_ratio`、`budget_min_per_second`）限制，以免后端吃力时重试成倍放大其负载。

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	connect_timeout 3s
	response_header_timeout 10s
	response_idle_timeout 30s
	max_conns 1000
	max_pending 5000
}

route {
	handler proxy {
		upstream app
		timeout 30s
		retry { attempts 2; statuses 502 503 }
	}
}
```

## 金丝雀发布和镜像

`upstreams` 按权重把路由的请求分给多个上游，`mirror` 把其中一部分复制一份发到别处，其应答被丢弃，也从不等待。

```fit
listener web :8080

upstream app 10.0.0.1:8080
upstream app-next 10.0.0.2:8080
upstream shadow 10.0.0.3:8080

route {
	handler proxy {
		upstreams { upstream app; weight 95 }
		upstreams { upstream app-next; weight 5 }
		mirror { upstream shadow; percent 10 }
	}
}
```

## HTTP/2、TLS、WebSocket 和 gRPC

上游默认使用 `http/1.1`，也可以是 `h2c`（明文 HTTP/2）或基于 TLS 的 `h2`，证书按系统根证书或 `tls_ca_file` 检查，后端要求时
出示客户端证书。WebSocket 升级，以及 HTTP/2 和 HTTP/3 的扩展 CONNECT，经同一路由转发到 HTTP/1.1 后端，空闲至多
`tunnel_idle_timeout`；gRPC 调用发往 `h2` 或 `h2c` 后端，连同 trailer 双向流式转发，受其 `grpc-timeout` 限制。

```fit
listener web :8443 http/1.1 h2 {
	tls certs/example.com.pem certs/example.com.key
}

upstream grpc {
	addresses 10.0.0.5:50051
	protocol h2c
}

upstream secure {
	addresses 10.0.0.6:443
	protocol h2
	tls_server_name api.internal
	tls_ca_file certs/ca.pem
}

route {
	match { header { Content-Type application/grpc* } }
	handler proxy { upstream grpc }
}

route { handler proxy { upstream secure } }
```

## 四层代理

`tcp` 或 `udp` 监听器和服务把连接和数据报逐字节代理给上游，使用同样的负载均衡和健康检查；一个 `tcp` 监听器上的多个服务可以按
TLS ClientHello 中的服务器名称共用它，并且不终止 TLS 直接透传。在 Linux 上，`tcp` 监听器的 `sockmap`（akit 自己的字段）在能
加载 akio 的 eBPF 中继时于内核中拼接这些连接。

```fit
listener db :5432 tcp { sockmap true }
listener tls :443 tcp

upstream postgres 10.0.0.5:5432
upstream mail 10.0.0.25:443
upstream web 10.0.0.26:443

service postgres { listeners db; upstream postgres; idle_timeout 30m }
service mail { listeners tls; upstream mail; server_names mail.example.com }
service web { listeners tls; upstream web; server_names www.example.com example.com }
```

## 来自别处的上游

上游及通往它们的路由可以来自 Docker 容器的标签、Consul 目录、Consul KV、etcd，或 Kubernetes 集群的 Gateway API 资源和
Ingress，并跟随其变化（[provider](../reference/config.zh-CN.md#provider)）。
