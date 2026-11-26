# 从 Caddy 迁移

[English](migrating-from-caddy.md) | [简体中文](migrating-from-caddy.zh-CN.md)

akit 与 Caddy 理念相同：默认 HTTPS、严格的结构化配置，以及原子地替换配置的 Admin API。不同之处在于内置了什么（缓存、限速、四层
代理，Caddy 把这些留给插件），以及每个连接的开销更小。它的 `.conf` 格式（即 fit 的格式）与 Caddyfile 一样由指令块写成，每组主机名
一个 `site`，但指令就是配置自己的字段。`akit adapt` 与 `fit adapt` 一样把 Caddyfile 中常见的部分转换过来；没有从 Caddy JSON 转换
的工具。[配置参考](../reference/config.zh-CN.md)列出每个字段。

## 一个站点

```caddyfile
example.com {
	encode zstd gzip
	handle_path /api/* {
		reverse_proxy 127.0.0.1:9000 127.0.0.1:9001 {
			lb_policy least_conn
			health_uri /healthz
		}
	}
	root * public
	file_server
}
```

由 `akit adapt -from caddyfile` 转换成下面的配置（Caddy 默认开启的 Admin API 变成 akit 的 `admin {}`）：

```fit
route example.com-api {
	match { host example.com; path /api/* }
	middleware compress { encodings zstd gzip }
	middleware rewrite { strip_prefix /api }
	handler proxy { upstream proxy_127_0_0_1_9000 }
}

route example.com {
	match { host example.com }
	middleware compress { encodings zstd gzip }
	handler static { root public }
}

upstream proxy_127_0_0_1_9000 127.0.0.1:9000 127.0.0.1:9001 {
	load_balancing least_conn
	health_check { path /healthz }
}

admin {}
```

手工用 site 可以写得更简洁：

```fit
upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
	load_balancing least_conn
	health_check {
		path /healthz
	}
}

site example.com {
	middleware compress
	route /api/* {
		middleware rewrite { strip_prefix /api }
		handler proxy { upstream app }
	}
	route /* {
		handler static { root public }
	}
}
```

与 Caddy 一样，不写监听器的配置在 443 上以 HTTPS 提供服务，证书由 Let's Encrypt 为路由的主机名签发，80 端口跳转过去。

## 转换 Caddyfile

```bash
akit adapt -from caddyfile -c Caddyfile -to akit -o akit.conf
akit validate -c akit.conf
```

它写出一个 `.conf` 文件，并在标准错误上列出略过的或转换时有疑问的指令及其行号。它转换全局选项（`email`、`acme_ca`、
`acme_ca_root`、`storage file_system`、`admin`、`auto_https`、`http_port`、`https_port`、`on_demand_tls`、`servers { protocols }`、
`log`、`debug`）、片段及其 `import`、环境变量占位符、按地址的站点、匹配器（路径、`path_regexp`、`host`、`method`、`header`、`query`、
`remote_ip`、`client_ip`、`not`），以及指令 `handle`、`handle_path`、`route`、`reverse_proxy`、`file_server`、`root`、`try_files`、
`respond`、`redir`、`rewrite`、`uri`、`header`、`request_header`、`encode`、`basic_auth`、`forward_auth`、`request_body`、
`php_fastcgi`、`tls` 和 `log`。

全局的 `log` 或 `debug` 会转换为 fit 的 `log`，即 akit 自身的日志，akit 与 fit 一样读取它（[参考](../reference/config.zh-CN.md#日志)）。站点中的
`log` 是访问日志。

## 各部分的对应

| Caddy | akit |
| --- | --- |
| 站点地址 `example.com` | `site example.com { ... }`，其路由匹配该主机；监听器由默认 HTTPS 生成，或手工写出 |
| `:8080` | 监听器：`listener web { address :8080; protocols http/1.1 h2c }` |
| 匹配器：`path`、`host`、`method`、`header`、`query`、`remote_ip`、`path_regexp`、`expression` | 路由的 `match { ... }`，字段同名，以及 `expr` |
| `handle`、`route` 及其顺序 | 路由，按 `priority`、然后最具体的主机和路径选择 |
| `handle_path /api/*` | `route /api/* { middleware rewrite { strip_prefix /api } ... }` |
| `reverse_proxy` | `handler proxy { upstream app }` 和 `upstream app { ... }` |
| `lb_policy` | 上游的 `load_balancing` |
| `health_uri`、`health_interval`、`fail_duration`、`max_fails` | `health_check`、`fail_timeout`、`max_fails` |
| `transport http { versions h2c }` | 上游的 `protocol`：`h2c`，或基于 TLS 的 `h2` |
| `file_server`、`root`、`browse`、`try_files`、`precompressed` | `static` 处理器的 `root`、`browse`、`try_files`、`precompressed` |
| `encode` | `compress` 中间件 |
| `header`、`request_header` | `headers` 中间件 |
| `redir` | `redirect` 中间件 |
| `rewrite`、`uri strip_prefix` | `rewrite` 中间件 |
| `respond` | `respond` 处理器 |
| `basicauth` | `basic_auth` 中间件，与 Caddy 一样使用 bcrypt 哈希 |
| `forward_auth` | `forward_auth` 中间件 |
| `php_fastcgi` | `fastcgi` 处理器，带 `split_path`、`index` 和 `try_files` |
| `tls cert.pem key.pem` | 监听器的 `tls.certificates` |
| `tls { on_demand }` 和 `on_demand_tls { ask ... }` | 监听器的 `tls.on_demand` 和 `acme.on_demand` |
| `email`、`acme_ca`、`storage`（Redis、Consul） | `acme` 的 `email`、`directory`、`storage`，以及用于 Redis、etcd 或 Consul 的 `shared_storage` |
| 站点中的 `log` | `access_log`，带 `output`、`level` 和 `format` |
| `metrics` | `metrics`，由 Admin API 在 `/metrics` 提供 |
| `tracing` | `telemetry`，经 OTLP |
| 插件：限速、缓存、四层代理 | 内置：`rate_limit`、`cache`、带服务的 `tcp` 和 `udp` 监听器 |
| `xcaddy build --with ...` | `akit build`（[模块](../modules.zh-CN.md)） |
| `caddy run --watch`、`caddy reload` | `akit run -watch`、SIGHUP 或 Admin API 的 `POST /load` |
| `caddy validate`、`caddy hash-password` | `akit validate`；哈希用 `htpasswd -B` 生成 |

## Admin API

akit 的 Admin API 在形态上与 Caddy 的相同：对 `/config/` 及其后的路径执行 `GET`、`PUT`、`PATCH` 和 `DELETE`，`POST /load`，以及
用 ETag 和 `If-Match` 防止更新丢失。路径是 JSON 指针的各段（`/config/routes/0/handler`），没有 `@id` 快捷方式；`PATCH` 是 JSON
merge patch；akit 保留已应用的配置（`GET /versions`），`POST /rollback/{n}` 再次应用其中之一。它监听 127.0.0.1:2019 或 Unix 套接字，
接受 bearer 令牌（`admin.token_file`）；尚不能经 TLS 提供（[Admin API](admin-api.zh-CN.md)）。

## 工作方式的不同

- **配置的形状。** akit 的配置顶层是监听器、路由、上游和服务，而不是 app 和 server；路由是一列中间件加一个处理器，而不是嵌套的
  handler。
- **改写不会重新选择路由。** `rewrite` 和 `uri` 在选定路由之后改写路径。
- **不提供以点开头的文件。** `static` 和 `fastcgi` 对路径中有以点开头的段答 404（`/.well-known/` 除外），而 Caddy 会提供它们；
  `dotfiles allow` 提供这些文件。
- **没有的**：Caddy 的内部 CA（`tls internal`）、DNS 验证（akit 应答 HTTP-01 和 TLS-ALPN-01）、`templates`、`map`、`handle_errors`
  以及 Caddy 的事件。以 `akit build` 编入的模块或 WebAssembly 插件代替插件。
