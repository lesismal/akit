# 指南

[English](README.md) | [简体中文](README.zh-CN.md)

如何用 akit 完成主要的工作，每篇指南都附有可以直接起步的配置。[配置参考](../reference/config.zh-CN.md)列出每个字段；这些指南中的
每个配置都由一个测试以 `akit validate` 检查。

- [入门](getting-started.zh-CN.md)：从零开始，到以 HTTPS 提供的站点，包括文件、被代理的应用和运行中修改配置。
- [反向代理与负载均衡](reverse-proxy.zh-CN.md)：上游、负载均衡、健康检查、超时与重试、金丝雀发布、WebSocket 与 gRPC、四层代理。
- [静态文件与 PHP](static-files.zh-CN.md)：站点、单页应用、压缩、PHP-FPM。
- [HTTPS 与证书](https.zh-CN.md)：默认 HTTPS、ACME、自己的证书和按需申请的证书、多个实例、客户端证书。
- [Admin API 与重载](admin-api.zh-CN.md)：SIGHUP、`-watch`、读取和修改运行中的配置、回滚、指标。
- [缓存](caching.zh-CN.md)：`cache` 中间件、磁盘层和清除。
- [WebAssembly 插件](wasm.zh-CN.md)：fit 的插件和 Proxy-Wasm 插件、它们的上限和开销。

从其他服务器迁移：

- [fit](migrating-from-fit.zh-CN.md)：akit 尚不运行的部分，以及需要留意的地方。
- [nginx](migrating-from-nginx.zh-CN.md)，借助 `akit adapt -from nginx`。
- [Caddy](migrating-from-caddy.zh-CN.md)，借助 `akit adapt -from caddyfile`。
- [Traefik](migrating-from-traefik.zh-CN.md)。
