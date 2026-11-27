# 管理多个 akit

[English](cluster.md) | [简体中文](cluster.zh-CN.md)

一个 akit（**管理端**）可以管理其他 akit（**成员**）：它把成员的 URL 和一个管理员的凭据保存在自己的加密存储中，每
`poll_interval` 读取一次每个成员的状态，并在控制台的集群页面中展示。成员不必为此改动：管理端使用成员已有的 API，即它的
控制台（用户名和密码）或它的 Admin API（token）。成员也可以是 fit：它们回答的是 fit 的协议
（[决定 041](../decisions/041-cluster-management.zh-CN.md)）。管理端不在数据路径上：管理端宕机只影响管理。

本指南介绍 akit 现在已经运行的部分（I130–I135）：成员及其状态、概览和节点页面、成员页面、节点切换器、分组和对多个成员的操作、模板。带健康门槛的发布、
漂移和告警、角色、NAT 之后的成员以及 `akit cluster` 命令随 I137–I139c 到来（[计划](../dev-plan.zh-CN.md#集群管理)）；
它们的菜单项已经画出，在那之前回答 "no such endpoint"。

## 管理端

`admin.cluster` 打开它。成员在控制台中展示，所以需要 `admin.console`；成员保存在存储中，由 `akit run` 检查：`admin.store`，
或 `akit run -admin` 在配置旁保存的存储。

```json
{
  "admin": {
    "console": {"users": [{"username": "ada", "password_hash": "$2b$10$..."}]},
    "store": {"path": "/var/lib/akit/akit.store", "password_env": "AKIT_STORE_PASSWORD"},
    "cluster": {"poll_interval": "10s", "timeout": "10s", "max_concurrency": 8}
  }
}
```

只有管理端、没有监听器和路由的配置是有效的：`akit run -admin -c manager.json`。

## 成员

每个 akit 都回答管理端读取的内容（I131）：`GET /node`（节点 ID、版本、运行中的配置和能力）、`/status` 和 `/schema`。
节点 ID 只生成一次，保存在存储中，或配置旁的 `akit.node-id`；在两者都无法持久的地方（例如配置只读的容器）设置
`admin.node_id`。

在管理端，**集群 → 节点 → 添加**：

- **通过控制台**：成员控制台的 URL 和它的一个用户。管理端登录并保持会话，会话结束时重新登录。被拒绝的密码在一分钟内
  不再尝试，此后加倍，最长 15 分钟，除非编辑或测试该成员，因此管理端不会把成员的用户锁在外面。
- **通过 Admin API**：成员 Admin API 的 URL 及其 token（`admin.token_file`）。

URL 为 `https`，或者只对回环地址使用 `http`，使凭据不以明文穿过网络。证书由系统 CA、成员自己的 CA 或固定的 SHA-256 信任：
**测试**显示成员出示的证书，确认其指纹即固定它。密码和 token 只写一次，只显示是否已设置，从不出现在回答或日志中，也只发给
其成员。

概览以颜色显示每个成员的状态：**up**、**error**（已到达，但证书、凭据或回答失败，各自区分）、**down**（未按时到达）或
**unknown**（尚未读取），以及版本、配置版本、连接、下线的上游后端、最近的证书到期时间和延迟。成员页面经管理端读取其节点、
状态、版本和一致性。

## 成员自己的页面

页头的节点切换器针对任一成员打开它自己的页面（配置及其文本标签、版本、引擎、角色页面）：管理端用它保存的会话或 token 转发每个
调用，浏览器从不持有成员的凭据。config schema 与管理端不同的成员（另一个 akit 版本，或 fit）只读打开；经切换器的每次写操作都
记入审计日志，带用户、前后的配置版本以及发送内容的摘要。

## 分组和操作

分组是一个标签选择器（`region=eu, tier!=db`，Kubernetes 的等值条件）、按名字列出的成员，或两者；其成员随当前标签变化。
**集群 → 分组**列出它们及每个分组的成员和状态，并对分组或选中的成员运行操作：**reload**、**rollback**（回滚到最近一个不同的
较早配置）、**check**、**purge** 缓存（按键、URL 前缀、标签或全部）和 **test**，同时最多 `max_concurrency` 个成员，各自回答。
下线的成员单独失败；每个操作连同每个成员的结果记入审计日志。

## 模板

模板拥有其成员配置中的部分：顶层成员整体拥有（`log`、`tls`），`listeners`、`routes`、`services` 和 `upstreams` 的元素按名字拥有。
其字符串可以使用 `${node.name}`、`${node.id}`、`${node.hostname}`、`${node.labels.<key>}` 以及所属分组的变量。**集群 → 模板**显示每个
成员的同步状态（已同步、未同步，或成员修改了被拥有部分而产生的冲突）及其期望配置，并能由成员的配置生成模板。每个成员都会被告知
管理端在其上拥有什么，并在自己的页面上标记这些字段。模板中的密钥显示为 `[secret]`；akit 成员的密钥按其 API 回答的样子（已脱敏）
比较。

## 混合集群

- akit 管理端同样管理 akit 和 fit 成员，通过控制台或 token；fit 成员的版本显示为 `fit <版本>`。
- fit 管理端通过 Admin API 的 token 管理 akit 成员。akit 的控制台只从 `X-Akit-Csrf` 读取 CSRF 令牌（决定 037），因此登录到
  akit 控制台的 fit 管理端能读不能写。
- akit 与 fit 的管理端共用存储：成员，以及 akit 管理端尚未用到的部分（发布），按原样保留。文档设置了用户角色的
  存储，在 I139 之前由 akit 管理端只读显示。

## 上限

`max_nodes` 个成员（256）；同时轮询 `max_concurrency` 个成员（8），每次调用在 `timeout` 之内、回答在 `max_response_bytes`
（4 MiB）之内；同时 4 个成员测试和 8 个成员读取；审计日志保留最近 1,000 次修改（约 2 MiB）。
