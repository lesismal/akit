# 缓存

[English](caching.md) | [简体中文](caching.zh-CN.md)

`cache` 中间件保存路由的响应并以它们应答，按 RFC 9111 共享缓存的方式工作：保存什么、保存多久，取决于响应的 `Cache-Control`、
`Expires`、`Vary` 和验证器。它把响应保存在内存中，放在指定同一 zone 的路由共享的 zone 里，设置 `disk` 时也写入磁盘。响应在发给
客户端的同时被保存，从不为了保存而扣留，流式响应体也是如此。[配置参考](../reference/config.zh-CN.md#cache)列出每个字段。

## 应用前面的缓存

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware cache {
		zone app
		max_bytes 512MiB
		max_entry_bytes 8MiB
		default_ttl 5m
	}
	handler proxy { upstream app }
}
```

它保存共享缓存可以保存的 `GET` 应答：不是 `no-store`、`private` 或 `no-cache`，不设置 cookie，不超过 `max_entry_bytes`，按
`s-maxage`、`max-age` 或 `Expires` 计算新鲜期，响应未说明且状态码允许时按 `default_ttl`。带 `Authorization` 的请求的响应，只有
在声明 `public`、`s-maxage` 或 `must-revalidate` 时才保存。成功的 `POST`、`PUT` 或 `DELETE` 会丢弃其 URI 已保存的内容。每个应答
都在 `Cache-Status`（RFC 9211）中说明缓存做了什么：命中、未命中、过期，或已保存。

zone 受 `max_bytes` 限制；超出时最久未用的响应被移除。缓存之后的 `compress` 中间件产生的每种编码，各是一个独立的变体。

## 大量客户端同时请求，以及上游失败

许多请求同时未命中同一响应时，一个去获取，其余等待它（缓存锁），这样一阵客户端只向上游发一个请求；`lock_timeout` 限制等待时间，
`disable_lock` 关闭它。`stale_while_revalidate` 在后台重新获取时立即以过期响应应答，`stale_if_error` 在获取失败时以过期响应应答，
用于响应本身未说明这些的情况（RFC 5861）。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /api/catalog/* }
	middleware cache {
		zone catalog
		max_bytes 256MiB
		lock_timeout 5s
		stale_while_revalidate 30s
		stale_if_error 10m
	}
	handler proxy { upstream app }
}
```

## 磁盘

`disk` 为 zone 增加磁盘层：保存在内存中的每个响应也在缓存自己的线程池上写入磁盘，内存不再保存的响应在其请求等待时从磁盘读回，
从不占用服务连接的线程。`max_bytes` 和 `max_entries` 限制它。

```fit
listener web :8080

upstream assets 127.0.0.1:9001

route {
	match { path /assets/* }
	middleware cache {
		zone assets
		max_bytes 256MiB
		default_ttl 1h
		disk { path cache/assets; max_bytes 10GiB; max_entries 1000000 }
	}
	handler proxy { upstream assets }
}
```

## 清除

Admin API 可以移除一个 zone 保存的内容：按 URL、按 URL 前缀、按标签（响应的 `Cache-Tag` 字段），或全部；`GET /cache` 说明每个
zone 保存了什么。

```bash
curl localhost:2019/cache
curl -X POST -H 'Content-Type: application/json' localhost:2019/cache/purge \
  -d '{"zone": "assets", "prefixes": ["https://example.com/assets/v1/"]}'
curl -X POST -H 'Content-Type: application/json' localhost:2019/cache/purge \
  -d '{"zone": "catalog", "tags": ["product-42"]}'
```

应答 `{"purged": n}` 说明移除了多少响应。
