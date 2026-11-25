# 入门

[English](getting-started.md) | [简体中文](getting-started.zh-CN.md)

本指南带你从零开始，到以 HTTPS 提供一个站点：包括文件、一个被代理的应用，以及在运行中修改配置。每一步都在一个配置文件上
增加内容；[配置参考](../reference/config.zh-CN.md)列出每个字段。akit 读取 fit 的配置，所以为
[fit](https://github.com/lesismal/fit) 写的配置无需修改就能在 akit 上运行，只有 akit 尚不支持的部分除外，akit 会按名称拒绝它们。

## 安装

用 Rust 1.85 或更高版本构建：

```bash
git clone https://github.com/lesismal/akit && cd akit
make build
```

生成的程序在 `target/release/akit`。打了标签的发行版还提供静态链接的 Linux 程序和 macOS 程序，仓库的 Dockerfile 可以构建容器
镜像；[部署](../../deploy/README.zh-CN.md)介绍了它们，以及 Helm chart 和 systemd 单元。`akit build` 可以构建带有你自己的模块的
akit（[模块](../modules.zh-CN.md)）。

```bash
akit version
```

## 第一个配置

配置可以是 `.conf` 文件、YAML 或 JSON；本指南使用 fit 自己的 `.conf` 格式，由指令块写成。把下面的内容保存为 `akit.conf`：

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

route hello {
	match {
		path /hello
	}
	handler respond {
		body "hello from akit\n"
	}
}
```

监听器说明 akit 在哪里监听、在那里使用什么协议。路由说明它接受哪些请求（`match`）、请求经过什么（`middleware`），以及由什么
应答（`handler`），每一项都是一个模块，其参数写在块中。先检查配置，再运行它：

```bash
akit validate -c akit.conf
akit run -c akit.conf
```

```bash
curl localhost:8080/hello
```

akit 严格检查配置：未知字段、类型错误或缺失的值都是错误，并给出其行和列，akit 不会启动。fit 读取而 akit 尚未读取的字段也一样：
报告 `not supported yet: <路径>`，从不读取后又忽略。没有路由匹配的请求得到 404。

## 提供文件

在第一条路由之后，加一条提供目录的路由：

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

route hello {
	match {
		path /hello
	}
	handler respond {
		body "hello from akit\n"
	}
}

route files {
	match {
		path /*
	}
	handler static {
		root ./public
	}
}
```

以 `*` 结尾的路径匹配前缀。多条路由都匹配时，最具体的胜出：无论书写顺序，`/hello` 先于 `/*`。`static` 对目录提供其
`index.html`；`browse` 列出没有 index 的目录，`try_files` 用于单页应用。文件在 akio 的文件池上读取，从不在服务连接的线程上。
[静态文件指南](static-files.zh-CN.md)有更多内容。

## 代理到应用

上游是一组有名字的后端；`proxy` 处理器把请求发给其中之一：

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
	health_check {
		path /healthz
		interval 5s
	}
}

route api {
	match {
		path /api/*
	}
	handler proxy {
		upstream app
	}
}

route files {
	match {
		path /*
	}
	handler static {
		root ./public
	}
}
```

到 `/api/` 的请求轮流发给两个后端，健康检查失败的后端被摘除，直到再次通过。WebSocket 和 gRPC 也以同样方式转发。每个连接、
队列和超时都有安全的默认值，可以用上游的字段修改。[反向代理指南](reverse-proxy.zh-CN.md)有更多内容。

## HTTPS

把路由写在以主机名命名的 site 中，不写监听器：

```fit
upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
}

site example.com {
	route /api/* {
		handler proxy { upstream app }
	}
	route /* {
		handler static { root ./public }
	}
}
```

site 的路由匹配它的主机名，并各自匹配自己的路径。这样 akit 在 443 上提供 HTTPS，证书由 Let's Encrypt 为 `example.com` 签发并
自动续期；在 80 上只用于跳转到 HTTPS。该名称的 DNS 必须指向本机，80 和 443 端口必须可达。`acme` 设置 CA 和证书的存放位置，
`auto_https` 设置端口或关闭这一功能。要使用你自己的证书，写出监听器，并在 site 中指定它：

```fit
listener web {
	address :443
	protocols http/1.1 h2 h3
	tls {
		certificates {
			cert_file certs/example.com.pem
			key_file certs/example.com.key
		}
	}
}

site example.com {
	listeners web
	route /* {
		handler static { root ./public }
	}
}
```

证书文件变化时，akit 会重新加载，而不关闭监听器。[HTTPS 指南](https.zh-CN.md)有更多内容。

## 中间件

中间件在处理器之前按顺序运行。在 site 中写的中间件作用于其每条路由；下面的配置压缩响应，并给每个响应加一个字段：

```fit
site example.com {
	middleware compress
	middleware headers {
		response {
			set {
				Strict-Transport-Security "max-age=31536000"
			}
		}
	}
	route /* {
		handler static { root ./public }
	}
}
```

[配置参考](../reference/config.zh-CN.md#中间件)列出全部中间件：改写和跳转、速率和并发限制、Basic 认证、JWT、forward auth、
客户端证书、熔断、缓存（[缓存](caching.zh-CN.md)）、WebAssembly 插件（[WebAssembly](wasm.zh-CN.md)）。

## 多个文件

配置变大时，把它拆开：`-c` 可以指定一个目录，其中（包括子目录中）每个 `.conf`、`.yaml`、`.yml` 和 `.json` 文件都会被读取，
各文件的路由和上游合并在一起：

```bash
akit run -c /etc/akit
```

例如 `/etc/akit/base.conf` 存放监听器和 `acme`，`/etc/akit/sites/` 中每个站点一个文件。列表之外的字段，如 `admin` 或 `acme`，
只在一个文件中给出。`-c` 也可以给出多次。

## 在运行中修改

`akit run` 在收到 SIGHUP 时重新应用配置；加上 `-watch` 时，配置的任一文件变化（包括目录中增加或移除文件）都会重新应用。无法
加载的配置不会替换正在运行的配置，并会说明原因。没有变化的监听器保留其连接，进行中的请求在它开始时的配置上完成。

配置中有 `admin {}` 时，Admin API（默认在 127.0.0.1:2019）以 fit 的原生 JSON 读取和修改运行中的配置：下面依次读取整个配置，
按 JSON 指针修改第一条路由的参数，用 `akit adapt` 把 `.conf` 文件转换后加载为全新的配置，再回滚到第一个版本：

```bash
curl localhost:2019/config
curl -X PATCH -H 'Content-Type: application/merge-patch+json' \
  -d '{"body": "hello again\n"}' localhost:2019/config/routes/0/handler/params
akit adapt -c akit.conf -o akit.json
curl -X POST -H 'Content-Type: application/json' -d @akit.json localhost:2019/load
curl -X POST localhost:2019/rollback/1
```

它保留已应用的配置，可以再次读取和回滚，并在 `/metrics` 提供指标、在 `/dashboard/` 提供仪表盘。
[Admin API 指南](admin-api.zh-CN.md)有更多内容。

## 接下来

- [配置参考](../reference/config.zh-CN.md)：每个字段、其类型和默认值，以及 akit 与 fit 的不同之处。
- [指南](README.zh-CN.md)：反向代理、静态文件、HTTPS、Admin API、缓存和 WebAssembly 插件。
- 从其他服务器迁移：[fit](migrating-from-fit.zh-CN.md)、[nginx](migrating-from-nginx.zh-CN.md)、
  [Caddy](migrating-from-caddy.zh-CN.md)、[Traefik](migrating-from-traefik.zh-CN.md)。
- [示例](https://github.com/lesismal/fit-examples)：fit 每个功能的配置；akit 运行其中它支持的那些。
- [部署](../../deploy/README.zh-CN.md)：发行包、容器镜像、Kubernetes 和 systemd。
