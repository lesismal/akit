# 从 nginx 迁移

[English](migrating-from-nginx.md) | [简体中文](migrating-from-nginx.zh-CN.md)

akit 保留了 nginx 的长处：事件循环和每个连接很小的开销；改变的是配置方式：一个结构化的配置，运行前先检查，并可经 Admin API
原地修改而无需重载。它的 `.conf` 格式（即 fit 的格式）与 nginx 一样由指令块写成，但指令就是配置自己的字段。本指南把 nginx 的指令
对应到 akit 的配置；[配置参考](../reference/config.zh-CN.md)列出每个字段。

## 先转换

`akit adapt` 与 `fit adapt` 一样转换 nginx 配置中常见的部分，并列出它略过的指令及其行号：

```bash
akit adapt -from nginx -c nginx.conf -to fit -o akit.conf
akit validate -c akit.conf
```

它转换 `http` 块中的 server（`listen`、`server_name`、`ssl_certificate`）、其中的 location（精确、前缀、`^~` 和正则表达式，可以
嵌套），以及其中的 `proxy_pass` 和 `upstream` 块、`root`、`alias`、`index`、`try_files`、`return`、`rewrite`、`limit_req` 及
`limit_req_zone`、`client_max_body_size`、`add_header`、值为字面量的 `proxy_set_header`、`gzip`，以及 `error_log` 和 `access_log`。
请阅读它的报告，并按下表手工补全配置。`auth_basic` 保护的内容在换上带有用户的 `basic_auth` 中间件之前一律答 401，这样原本需要
密码的内容不会被任何人取得。

`error_log` 会转换为 fit 的 `log`，而 akit 尚不支持它：删除转换器写出的 `log`，akit 会把自己的消息写到标准输出和标准错误。

例如：

```nginx
events {}
http {
    upstream app {
        least_conn;
        server 127.0.0.1:9000;
        server 127.0.0.1:9001;
    }
    server {
        listen 8080;
        server_name example.com;
        gzip on;
        client_max_body_size 10m;
        location /api/ {
            proxy_pass http://app;
        }
        location / {
            root public;
            try_files $uri $uri/ /index.html;
        }
    }
}
```

会转换成下面的配置，并对没有对应物的 `events` 给出警告：

```fit
listener http_8080 :8080 http/1.1

upstream app 127.0.0.1:9000 127.0.0.1:9001 { load_balancing least_conn }

route example.com-api {
	listeners http_8080
	match { host example.com; path /api/* }
	middleware body_limit { max_bytes 10MiB }
	middleware compress
	handler proxy { upstream app }
}

route example.com {
	listeners http_8080
	match { host example.com; path /* }
	middleware body_limit { max_bytes 10MiB }
	middleware compress
	handler static { root public; try_files {path} {path}/ /index.html }
}
```

手工整理时，用 `site` 可以把同样的路由写得更简洁。

## 各部分的对应

| nginx | akit |
| --- | --- |
| `server { listen 8080; }` | 监听器：`listener web { address :8080; protocols http/1.1 h2c }` |
| 带 `ssl_certificate` 和 `ssl_certificate_key` 的 `listen 443 ssl http2;` | `protocols http/1.1 h2` 且带 `tls { certificates { cert_file ...; key_file ... } }` 的监听器；加上 `h3` 即提供 HTTP/3 |
| `server_name a.example *.example;` | `site a.example *.example { ... }`，或路由的 `match.host`；`*.` 代表一级，`**.` 代表一级或多级 |
| `location = /x` | `match.path: ["/x"]` |
| `location /x/` | `match.path: ["/x/*"]` |
| `location ~ \.php$` | `match.path_regexp: ["(?i)\\.php$"]` |
| `proxy_pass http://app;` 和 `upstream app { server ...; }` | `handler proxy { upstream app }`，以及 `upstream app { addresses ... }` |
| `least_conn;`、`ip_hash;`、`hash $cookie_x;` | 上游的 `load_balancing`：`least_conn`、`ip_hash`，以及带 `hash_key` 的 `cookie_hash` |
| `server ... weight=3 max_fails=2 fail_timeout=10s;` | `weights`、`max_fails`、`fail_timeout` |
| `server ... backup;` | proxy 的 `failover`：另一个上游，在第一个上游全部不可用时使用 |
| `proxy_connect_timeout`、`proxy_read_timeout` | `connect_timeout`、`response_header_timeout` 和 `response_idle_timeout` |
| `keepalive 32;` | `max_idle_conns`（默认 32） |
| `proxy_set_header`、`add_header` | `headers` 中间件的 `request` 和 `response` |
| `root`、`alias`、`index`、`try_files`、`autoindex on;` | `static` 处理器的 `root`、`strip_prefix`、`index`、`try_files`、`browse` |
| `return 301 https://$host$request_uri;` | `redirect` 中间件：`to https://{host}{uri}; status 301` |
| `rewrite ^/old/(.*)$ /new/$1;` | `rewrite` 中间件的 `regexp` 和 `replacement` |
| `return 200 "ok";` | `respond` 处理器 |
| `gzip on;`、`gzip_comp_level`、`gzip_min_length` | `compress` 中间件，还支持 zstd 和 brotli |
| `limit_req_zone` 和 `limit_req` | `rate_limit` 中间件：`rate`、`burst`、`key` |
| `limit_conn` | `concurrency_limit` 中间件 |
| `client_max_body_size 10m;` | `body_limit` 中间件，或作用于所有监听器的 `limits.max_body_bytes` |
| `auth_basic` 和 `auth_basic_user_file` | `basic_auth` 中间件，使用 bcrypt 哈希（`htpasswd -B`） |
| `auth_request` | `forward_auth` 中间件 |
| `proxy_cache_path` 和 `proxy_cache` | `cache` 中间件：`zone`、`max_bytes`，以及磁盘层 `disk`（[缓存](caching.zh-CN.md)） |
| 带 `fastcgi_param` 的 `fastcgi_pass` | `fastcgi` 处理器，PHP 的参数会自动设置 |
| `stream { server { listen 5432; proxy_pass db; } }` | `tcp`（或 `udp`）监听器，以及指定它和上游的服务 |
| `ssl_preread` 和 `map $ssl_preread_server_name` | 服务的 `server_names`，按 SNI 透传 TLS |
| `access_log /var/log/access.log buffer=32k flush=5s;` | `access_log` 的 `output`、`buffer_bytes` 和 `flush_interval`；日志行为 JSON、text 或 line，字段是自己的 |
| `nginx -s reload` | SIGHUP、`akit run -watch`，或 Admin API 的 `POST /load` |

## 工作方式的不同

- **哪条路由匹配。** nginx 先试精确的 location，再试最长前缀，再按顺序试正则表达式。akit 先按 `priority`，然后是最具体的主机，
  然后是路径，然后是其他匹配条件最多的路由；正则表达式与其他匹配条件一样。要让某条路由优先，给它设置 `priority`。
- **没有变量。** akit 没有 `$变量` 和 `if`。匹配用 `match`（主机、路径、方法、头部、查询参数、远端地址，或组合它们的 `expr`）；
  跳转和改写用它们自己的占位符（`{host}`、`{uri}`、`$1`）。
- **默认 HTTPS。** 没有监听器、路由带主机名的配置，在 443 上以 Let's Encrypt 的证书提供 HTTPS，80 上的 HTTP 跳转过去。要保持
  nginx 的行为，写出监听器，或设置 `auto_https.disable`。
- **健康检查和会变化的上游。** 内置主动健康检查（`health_check`），上游可以来自 Docker、Consul、etcd 或 Kubernetes（`providers`）。
- **上限默认开启。** 上游的连接、队列和超时、请求体，以及未被读取的响应，都有安全的默认值。
- **不提供以点开头的文件。** `static` 和 `fastcgi` 对路径中有以点开头的段答 404，相当于 `location ~ /\. { deny all; }`，
  `/.well-known/` 除外。
- **不转换的**：Lua、`njs`、`map`、`geo`、`split_clients`、`mirror`（proxy 有自己的 `mirror`）、`sub_filter`、邮件代理以及 nginx
  的模块。WebAssembly 插件和以 `akit build` 编入的模块代替这些扩展。
