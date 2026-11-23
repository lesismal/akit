# akit

[English](README.md) | [简体中文](README.zh-CN.md)

akit 是用 Rust 基于 [akio](https://github.com/lesismal/akio) 编写的反向代理、负载均衡、TLS 终结、静态文件服务和
API 网关，定位与 nginx、Caddy、Traefik 和 [fit](https://github.com/lesismal/fit) 相同。它是在 akio 上重建的 fit，
akio 是 fit 的引擎 [fib](https://github.com/lesismal/fib) 在 Rust 中的对应物：功能相同，配置文件相同、三种格式相同，
Admin API 相同；没有垃圾回收，回调直接运行在每核一线程的 I/O 线程上，每个监听器各自选择 io_uring 或 epoll，自身没有
一行 `unsafe`。

**状态：规划中。** 还没有构建任何东西。计划是逐个里程碑达到 fit 1.0 的功能集，然后再做 fit 在边缘代理之外的各条
路线。

## 文档

- [路线图](docs/roadmap.zh-CN.md)：目标、设计原则、里程碑，以及与 fit 对照的功能地图。
- [开发计划](docs/dev-plan.zh-CN.md)：拆分成迭代的里程碑，每个迭代一次会话。
- [开发进度](docs/progress.zh-CN.md)：已完成的内容，以及从哪里继续。
- [架构](docs/architecture.zh-CN.md)：crate、线程、运行时图，以及一个请求经过的路径。
- [开发规则](docs/rules.zh-CN.md)：每次改动都必须遵守的规则。
- [akio API 需求](docs/akio-api-requests.zh-CN.md)：akit 需要而 akio 还没有的东西。
- [决定](docs/decisions/README.zh-CN.md)：依赖策略、与 fit 的配置兼容，以及之后做出的其他决定。

## 预期的样子

fit 能运行的配置，例如 fit 的 [hello](https://github.com/lesismal/fit-examples/blob/main/hello/fit.conf)，
应当不加修改就能运行：

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

```bash
akit validate -c hello.conf
akit run -c hello.conf
```

## 构建

I01 完成后：使用 akio 的 MSRV 对应的 Rust，`cargo build --release`，用 `make check` 运行 fmt、clippy、cargo-deny 和
测试。Linux 是一等平台；macOS 用于开发；不支持 Windows。

## 许可证

Apache License 2.0，见 [LICENSE](LICENSE)。
