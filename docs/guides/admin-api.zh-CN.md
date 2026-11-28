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

API 默认监听 127.0.0.1:2019，也可以监听 Unix 套接字（`unix//run/akit/admin.sock`，权限设为 0600）。设置 `token_file`
时，每个请求都要以 `Authorization: Bearer <令牌>` 带上令牌。设置 `cert_file` 和 `key_file` 时经 TLS（1.2 或更高）提供，
再设置 `client_ca_file` 时只服务证书链到其中某个 CA 的客户端。非回环地址既要令牌也要 TLS，使令牌从不以明文经过网络；
没有它们时，请在本机、经 SSH 隧道或用 `docker exec` 访问 API。它的设置在 akit 启动时读取，但 `access` 随每次应用的配置
变化，证书文件也不同：带 `-watch-certs` 时每个 `-watch-interval` 查看一次，SIGHUP 也会重新读取，在 akit 之外续期的证书
无需重启即提供给新连接。加载失败的文件（例如证书已写入而密钥尚未写入）记录到 `admin` 日志器，正在使用的证书保留到文件
再次变化。

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
| `GET /logs/recent?level=&logger=&contains=&limit=&after=` | akit 自身日志的最新消息，取自内存中保留的最近 1,000 条，最旧的在前，`after` 之后编号的消息；集群管理端跨成员跟踪它们 |
| `POST /config/render?format=`、`POST /config/parse?format=` | 把一个配置片段（JSON 对象）写成 `.conf`、`yaml` 或 `json`；以及这样一段文本的 JSON |
| `PUT /config/text?format=&members=` | 把一段文本作为整个配置，或只作为其中那些顶层成员应用，其余保留；`If-Match` 和 `dry_run` 同 `PUT /config`；其错误定位到文本中的行 |
| `POST /config/export`、`POST /config/import?format=&members=` | 配置（不含 `admin`，秘密被遮蔽）或其部分成员导出为文件；以及应用一个文件，保留 `admin` |
| `GET /dashboard/` | 状态和配置的只读仪表盘 |
| `GET /tokens`、`GET` 和 `POST /tokens/{成员}`、`PATCH` 和 `DELETE /tokens/{成员}/{id}` | 隧道 hub 和 LLM 角色的监听方签发的链路 token（`tunnel.hub`、`llm.hub`、`llm.node`、`llm.proxy`）：列出（带过期时间、最后使用、成员和链路）；签发（`{name, note, ttl \| expires_at}`，token 只显示一次，形如 `akitt1.<id>.<secret>`）；修改过期时间；撤销。每个修改都是对成员的一次编辑，记录到 `admin` 日志器；`akit token` 调用它们 |
| `/bittorrent/...` | BitTorrent 角色的种子、事件和统计（[见下](#bittorrent)）；`akit bt` 调用它们 |
| `/cluster/...` | 集群管理端（`admin.cluster`）上的成员、状态、分组、漂移、差异、序列、证书、日志和发布，以名为 `Admin API` 的集群范围管理员身份行事（[集群指南](cluster.zh-CN.md#akit-cluster)）；`akit cluster` 调用它们；不管理成员的 akit 回答 404 |

带 `If-Match: <etag>` 的修改，若配置在读取之后已被改变，则以 412 拒绝，这样两个客户端不会互相覆盖。带 `?dry_run=1` 时，修改
走完应用的完整流程，像真正应用一样应答，但不应用任何东西。

配置中内联写出的秘密，即成员名表明其为秘密的值（`password_hash`、`secret`、`token`、路由设置的 `Authorization` 头），在每次
读取时都以 `[redacted]` 给出：`GET /config` 及其指针、`GET /versions/{n}` 和 `GET /config/effective`，与 fit 控制台的显示
相同。修改中送回的 `[redacted]` 保留运行中配置在同一位置的秘密，所以读出、编辑、再加载的配置不会丢失秘密；没有秘密的位置上
出现 `[redacted]` 以 400 拒绝。要修改秘密，就送新值。保存在文件中的秘密（`token_file`、`password_file`、`key_file`）从不在
配置中；这样的文件若其组或其他用户可读，akit 在启动时和每次修改时都会警告（[决定 035](../decisions/035-admin-api-redacts-secrets.zh-CN.md)）。

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

`admin.access` 指定 API 为哪些客户端地址提供服务，在全局 `access` 之前决定，全局 `access` 也适用于 API
（[访问控制列表](../reference/config.zh-CN.md#访问控制列表)）；与 `admin` 的其余部分不同，它随每次应用的配置生效。不包含
`127.0.0.1` 的全局 `allow` 也会把本地客户端挡在 API 之外，除非 API 自己的列表放行它们：

```fit
access { allow 10.0.0.0/8 }

admin {
	address 127.0.0.1:2019
	access { allow 127.0.0.1 ::1 }
}

route { match { host example.com }; handler respond { body hello } }
```

## 链路 token

隧道的 hub 和 LLM 角色的监听方接受持有其签发 token 的成员的链路，以代替所有成员共享的密码（fit 的决定 035，
[决定 038](../decisions/038-tunnel-and-llm.zh-CN.md)）。token 形如 `akitt1.<id>.<secret>`：配置中保存其 ID、名称、时间、
备注、绑定到它的成员和一个哈希，从不保存 token 本身，token 只显示一次。`akit token` 通过运行中 akit 的 Admin API 管理它们，
其地址、令牌和 TLS 从配置的 `admin` 读取：

```sh
akit token create -c akit.json -role tunnel.hub -name lab -ttl 90d   # 打印 token，仅此一次
akit token list   -c akit.json -role tunnel.hub                      # 过期时间、最后使用、成员、链路
akit token expiry -c akit.json -role tunnel.hub -id fdd44d906b8e6ce2 -never
akit token revoke -c akit.json -role tunnel.hub -id fdd44d906b8e6ce2
```

fit 签发的 token 换上 akit 的前缀就是同一个 token：`fitt1.<id>.<secret>` 写作 `akitt1.<id>.<secret>`。与 fit 不同，无论其
角色是否已在运行，这些端点都作用于运行中配置所设置的成员；运行中的角色会补上每个 token 的最后使用和链路。

## BitTorrent

`bittorrent.client` 运行时，API 在 `/bittorrent/` 下提供它的种子（fit 的 `/bittorrent/...`，
[决策 042](../decisions/042-bittorrent.zh-CN.md)）；`bittorrent.tracker` 运行时提供 tracker 的统计。未运行的角色的端点以 404
说明这一点。

| 端点 | 作用 |
| --- | --- |
| `GET /bittorrent/torrents` | 按队列顺序列出种子，`{torrents, total, offset}`；`state`（逗号分隔的状态）、`category`、`q`（名称中包含）、`offset`、`limit`（默认 1,000，至多 10,000） |
| `POST /bittorrent/torrents` | 添加一个：JSON `{magnet \| url \| torrent（base64）, category, paused, peers, limits}`；以 `application/x-bittorrent` 发送 .torrent 文件，`category` 和 `paused` 放在查询中；或表单 `multipart/form-data`，文件在 `torrent` 部分。返回 201 和 `Location` |
| `GET`、`POST`、`PATCH`、`DELETE /bittorrent/torrents/{id}` | 一个种子及其文件；`{"action": "pause" \| "resume" \| "recheck"}`；修改 `paused`、`category`、`limits`（`null` 清除）、`queue_position`；删除，`?delete_data=1` 连同文件 |
| `GET /bittorrent/torrents/{id}/files`、`/peers`、`/trackers` | 它的文件、peer、tracker 和 web seed |
| `GET`、`PUT /bittorrent/alt_speed` | 备用速率；`{"mode": "auto" \| "on" \| "off"}` |
| `GET /bittorrent/publish`、`/publish/{name}`、`/publish/{name}/torrents/{id}` | 发布条目、其中一个，以及它发布的一个 .torrent 文件 |
| `GET /bittorrent/events` | 客户端的事件，以 server-sent events 推送（`added`、`removed`、`state`、`completed`、`error`、`seed_limit`、`alt_speed`）；`Last-Event-ID`（或 `?after=`）重放最近 1,024 个中错过的事件，有些已不在时先发 `reset`；每 15 秒一条注释；至多 32 个流 |
| `GET /bittorrent/tracker/stats`、`/tracker/torrents`、`/tracker/torrents/{hash}` | tracker 的总数、允许列表中的 swarm、一个 swarm |

种子的 `{id}` 是其 v1 info hash 的十六进制，或其 v2 hash（64 个十六进制数字）。`GET /metrics` 包含客户端的
`akit_bittorrent_*` 指标族（按分类的字节数、速率、peer 和各状态的种子数，备用速率，事件流，已发布的种子）以及 tracker 的
`akit_bittorrent_tracker_*`。`akit bt` 调用这些端点，就像 `akit token` 调用它自己的端点：

```sh
akit bt add -c akit.json -category tv ubuntu.torrent 'magnet:?xt=urn:btih:...'
akit bt ls -c akit.json -state downloading,seeding
akit bt info -c akit.json ubuntu        # 按名称、ID，或 ID 的至少 4 个十六进制数字开头
akit bt limit -c akit.json -up 512K -seed-time 12h ubuntu
akit bt alt -c akit.json on
akit bt rm -c akit.json -delete-data ubuntu
```

akio 尚不能做的事按名称以 501 拒绝：顺序下载和文件优先级（akio 的 A287）、种子的 `max_conns` 和 `upload_slots`（A284）。在
akio 的 A288 之前，种子的 tracker 以未知状态列出，重新校验则不带 resume 数据重新添加种子：它校验每个分片，并重新连接其 peer。
fit 的 `fit bt rss` 随 RSS 一起提供。

## 指标和追踪

`GET /metrics` 提供 akit 的指标：上游及其地址、证书、连接、重载、akio 引擎，以及（设置 `metrics {}` 时）按监听器和路由统计的
请求。`telemetry` 经 OTLP/HTTP 把请求的追踪和同样的指标导出到 collector（[遥测](../reference/config.zh-CN.md#遥测)）。

## 尚不支持

fit 的助手尚不支持：akit 按名称拒绝它。[控制台](console.zh-CN.md)、加密的 store（`admin.store`，`akit run`
把配置保存在其中，以及 `/config/export` 和 `/config/import` 的 `store` 格式）和 `akit run -admin` 已支持。
