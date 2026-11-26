# Admin API 与重载

[English](admin-api.md) | [简体中文](admin-api.zh-CN.md)

运行中的 akit 有三种方式接受新配置，应用的方式都相同：SIGHUP 重新读取配置文件，`akit run -watch` 在任一文件变化时重新读取，
Admin API 经 HTTP 接受一个配置或对配置的修改。无法加载的配置不会替换正在运行的配置，并会说明原因。没有变化的监听器保留其连接；
变化了的监听器重新绑定，旧的排空；进行中的请求在它开始时的配置上完成。[配置参考](../reference/config.zh-CN.md#admin-api)列出
每个字段。

## 重载

```bash
akit run -c /etc/akit
kill -HUP "$(pidof akit)"
akit run -watch -c /etc/akit
```

加上 `-watch` 时，配置的某个文件被修改、其目录中增加或移除了文件，都会重新应用配置，每隔 `-watch-interval`（1s）检查一次。证书
文件单独监视（`-watch-certs`，默认开启）。SIGINT 和 SIGTERM 让 akit 平稳停止，进行中的请求至多可以再完成 `-shutdown-timeout`
（30s）。

## Admin API

```fit
admin {
	address 127.0.0.1:2019
	token_file admin.token
}

metrics {}

listener web :8080

route hello {
	match { path /hello }
	handler respond { body "hello\n" }
}
```

API 默认监听 127.0.0.1:2019，也可以监听 Unix 套接字（`unix//run/akit/admin.sock`，权限设为 0600）。akit 还不能经 TLS 提供
API，所以不接受其他地址：请在本机、经 SSH 隧道或用 `docker exec` 访问它。设置 `token_file` 时，每个请求都要以
`Authorization: Bearer <令牌>` 带上令牌。它的设置在 akit 启动时读取。

它提供：

| 请求 | 作用 |
| --- | --- |
| `GET /config`、`GET /config/<指针>` | 以 fit 原生 JSON 给出运行中的配置，或 JSON 指针所指的值，带 `ETag` |
| `POST /load` | 一个完整的配置，使用 fit 的原生 JSON（其他格式由 `akit adapt` 转换） |
| `PUT`、`PATCH`、`DELETE /config/<指针>` | 修改指针所指的值；`PATCH` 接受 JSON merge patch |
| `POST /reload` | 把运行中的配置作为新版本重新应用：证书文件等会重新读取 |
| `GET /versions`、`GET /versions/{n}`、`POST /rollback/{n}` | 已应用的配置、其中之一，以及回滚到它（作为新版本应用） |
| `GET /config/effective` | 合并了 provider 部分的配置，以及每一部分的来源 |
| `GET /status` | 监听器、上游及其地址的健康状况、证书、重载 |
| `GET /metrics` | Prometheus 文本格式的指标 |
| `GET /cache`、`POST /cache/purge` | 每个缓存 zone 保存了什么，以及清除（[缓存](caching.zh-CN.md)） |
| `GET /logs`、`POST /logs/rotate`、`POST /logs/cleanup` | `log` 和 `access_log` 的日志文件及其切割设置和磁盘上的文件，立即切割其中一个，以及按保留策略删除的文件：删除，或（`dry_run`）只列出 |
| `GET /dashboard/` | 状态和配置的只读仪表盘 |

带 `If-Match: <etag>` 的修改，若配置在读取之后已被改变，则以 412 拒绝，这样两个客户端不会互相覆盖。带 `?dry_run=1` 时，修改
走完应用的完整流程，像真正应用一样应答，但不应用任何东西。

```bash
TOKEN=$(cat admin.token)
curl -H "Authorization: Bearer $TOKEN" localhost:2019/config/routes/0
curl -H "Authorization: Bearer $TOKEN" -X PATCH \
  -H 'Content-Type: application/merge-patch+json' \
  -d '{"body": "hello again\n"}' localhost:2019/config/routes/0/handler/params
curl -H "Authorization: Bearer $TOKEN" -X POST 'localhost:2019/load?dry_run=1' \
  -H 'Content-Type: application/json' -d @akit.json
curl -H "Authorization: Bearer $TOKEN" localhost:2019/versions
curl -H "Authorization: Bearer $TOKEN" -X POST localhost:2019/rollback/1
```

经 API 加载的配置不会写回文件：之后的 SIGHUP 会再次应用文件中的配置。要么以文件为准，要么每次修改都经 API 加载。

## 指标和追踪

`GET /metrics` 提供 akit 的指标：上游及其地址、证书、连接、重载、akio 引擎，以及（设置 `metrics {}` 时）按监听器和路由统计的
请求。`telemetry` 经 OTLP/HTTP 把请求的追踪和同样的指标导出到 collector（[遥测](../reference/config.zh-CN.md#遥测)）。

## 尚不支持

fit 的控制台、加密的 store、助手、集群管理、Admin API 的 TLS 和访问列表，以及 `fit run -admin` 尚不支持：akit 按名称拒绝它们。
