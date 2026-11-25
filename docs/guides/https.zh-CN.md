# HTTPS 与证书

[English](https.md) | [简体中文](https.zh-CN.md)

akit 以来自文件的证书、以 ACME（默认 Let's Encrypt）申请并续期的证书，或两者一起提供 HTTPS。每次握手按客户端请求的名称选择
证书；证书文件变化时重新加载，ACME 的证书自动续期，都不关闭监听器。申请证书以及等待证书的握手，从不占用服务连接的线程。
[配置参考](../reference/config.zh-CN.md#acme)列出每个字段。

## 默认 HTTPS

不写监听器、且路由匹配主机名的配置，在 443 上以 HTTPS 提供服务，为每个名称申请证书；并在 80 上提供 HTTP，用于跳转到 HTTPS
（308）和应答 ACME 的 HTTP-01 验证：

```fit
acme { email ops@example.com }

site example.com www.example.com {
	route /* {
		handler static { root public }
	}
}
```

这些名称的 DNS 必须指向本机，80 和 443 端口必须可达。证书保存在 `acme.storage`，默认为 `~/.local/share/akit`，按其有效期或 CA
的续期信息（ARI）续期，并装订 OCSP 响应。公共 CA 不签发证书的名称（`localhost`、IP 地址、`*.internal`）不会获得证书。
`auto_https` 可以改变端口、关闭跳转，或整体关闭：

```fit
auto_https { http_port 8080; https_port 8443 }

route { match { host example.com }; handler respond { body hello } }
```

先用 CA 的测试目录试一试，它的限制宽松：
`acme { directory https://acme-staging-v02.api.letsencrypt.org/directory }`。

## 自己写监听器

写一个带 `tls` 的监听器，以选择它的地址和协议。它的 `acme` 列出要申请证书的名称；`certificates` 指定证书文件；空的 `tls {}`
获取该监听器的路由所匹配的主机名的证书。

```fit
listener https :443 http/1.1 h2 h3 {
	tls {
		acme example.com www.example.com
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
	}
}

listener http :80

route {
	listeners https
	handler static { root public }
}

route {
	listeners http
	middleware redirect { to https://{host}{uri}; status 308 }
	handler respond
}
```

`h3` 在同一端口的 UDP 上增加 HTTP/3，并以 `Alt-Svc` 告知 HTTP/1.1 和 HTTP/2 客户端。`max_version 1.2` 让监听器停留在 TLS 1.2。

## 按需申请证书

对于事先不知道的名称，例如平台客户的域名，`on_demand` 在第一次请求某个名称的握手中申请证书，前提是 `ask` URL 允许该名称
（应答 2xx），并且每 `interval` 至多 `rate` 个：

```fit
acme {
	email ops@example.com
	on_demand {
		ask http://127.0.0.1:5555/allowed
		rate 10
		interval 1m
	}
}

listener https :443 { tls { on_demand true } }

route { handler respond { body hello } }
```

## 多个实例

多台主机上的实例经 `shared_storage` 在 Redis、etcd 或 Consul 中共享证书：一个实例持锁申请证书，其他实例取用；每个验证的应答
也保存在那里，因此 CA 访问到哪个实例，哪个实例就能应答。

```fit
acme {
	email ops@example.com
	shared_storage {
		etcd { endpoints http://etcd-1:2379 http://etcd-2:2379 }
	}
}

route { match { host example.com }; handler respond { body hello } }
```

## 客户端证书

`client_auth` 要求客户端出示证书，并按 `ca_files` 中的 CA 检查（双向 TLS）：`require` 在握手中拒绝没有证书的客户端；
`verify_if_given` 允许其连接，由 `client_cert` 中间件在需要证书的路由上要求证书，并把证书主题传下去。

```fit
listener internal :8443 http/1.1 h2 {
	tls {
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
		client_auth { ca_files certs/ca.pem; mode require }
	}
}

route { handler respond { body "hello, certified client\n" } }
```
