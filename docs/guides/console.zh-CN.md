# 控制台

[English](console.md) | [简体中文](console.zh-CN.md)

控制台是一组需要用户名和密码登录的网页，从中读取和修改运行中的配置：即 fit 的控制台，akit 用 fit 自己的页面提供它。它是
`admin.console`，与 [Admin API](admin-api.zh-CN.md) 并列，在自己的登录之后于进程内调用 Admin API 的端点，因此在控制台中做的
修改与其他配置走同一条应用路径、经过同样的检查，并以控制台为来源记入 `/versions`。[配置参考](../reference/config.zh-CN.md#控制台)
列出所有字段；[决定 036](../decisions/036-console.zh-CN.md) 说明 akit 与 fit 的不同之处。

## 配置

用户是一个名字和一个 bcrypt 哈希；密码从不以明文写出。`akit hash-password` 打印从标准输入读取的密码的哈希（`-cost` 设置
bcrypt 的代价，默认 10；密码最多 72 字节）。它从不从参数中读取密码，因为本机的其他用户能看到参数。从终端输入时它询问两次，
但还不能隐藏输入的内容（akio 的 A233）；要隐藏：

```bash
read -rs p; printf '%s\n' "$p" | akit hash-password
```

fit 的哈希原样可用。然后：

```fit
admin {
	address 127.0.0.1:2019
	console {
		address 127.0.0.1:2020
		users { username admin; password_hash "$2b$10$r4CnBqUvPHMEJ2c8gc9SsOZeUyDeOB3PHsKffYxFv2ckCM4qrof.C" }
	}
}

listener web :8080 http/1.1 h2c
route { handler respond { body "hello\n" } }
```

再打开 <http://127.0.0.1:2020/>。只有 `admin`、没有监听器的配置是有效的：其余部分可以在控制台中添加。

**非回环地址上的控制台需要 TLS**：非回环地址必须有 `cert_file` 和 `key_file`，使密码从不以明文经过网络；没有时
`akit validate` 拒绝。在回环地址上 TLS 可选，比如通过 SSH 隧道访问的控制台。`access` 指定它为哪些客户端地址提供服务
（[访问控制列表](../reference/config.zh-CN.md#访问控制列表)），在全局 `access` 之前决定，全局 `access` 也适用于控制台：
`allow 127.0.0.1 ::1` 不论全局访问控制如何都保留本机访问。与 `admin.console` 的其余部分（在 akit 启动时读取）不同，`access`
随每次应用的配置变化。

## 登录与会话

- **检查密码。** 不是用户的名字用一个假哈希检查，使耗时不能区分名字；同时最多检查两个，在单独的池上进行（一个运行、16 个等待，
  再多则 `503`），从不在控制台的 I/O 线程上。
- **锁定。** 一个客户端地址 15 分钟内失败 5 次即被拒绝 15 分钟，回答 `429` 和 `Retry-After`；在安静 15 分钟之前再次被锁定时，
  时长加倍，最长一小时。IPv6 地址按 /64 计算。一个地址同一时间只检查一个；最多记录 4,096 个地址。
- **延缓的用户。** 一个用户的密码（来自任何地址）连续失败 10 次后，其检查被拒绝 2 秒，此后每再失败一次时长加倍，最长 5 分钟。
- **会话。** 登录得到一个会话：cookie `fit_console_<端口>` 中的 32 个随机字节，`HttpOnly`、`SameSite=Strict`，TLS 上还有
  `Secure`。控制台只保存其 SHA-256，在 `session_timeout`（30m）内没有请求、总计超过 `session_lifetime`（12h）、登出，或保存了
  `max_sessions`（64）个会话时作为最旧的一个结束它。会话保存在内存中：重启会让所有人登出。
- **其他网页。** 除读取外的每个请求都在 `X-Fit-Csrf` 中携带会话的 CSRF 令牌，否则得到 `403`；`Origin` 不是控制台自己的请求
  得到 `403`。回答带有只允许页面自身文件和 `frame-ancestors 'none'` 的 Content-Security-Policy、`X-Frame-Options: DENY`、
  `Referrer-Policy: no-referrer`，TLS 上还有 `Strict-Transport-Security`；API 的回答带 `Cache-Control: no-store`。

`admin` 日志记录器记录谁从哪里登录、每次失败的登录、每次登出，以及每个用户的每次修改及其应用的配置版本。

**修改密码需要 store。** fit 把修改后的密码保存在 `admin.store` 中，akit 还没有它（[I93](../dev-plan.zh-CN.md#i93-加密的-store)）：
在此之前相应端点回答 `409`，用户即配置中的用户。

## 供脚本使用

控制台自己的端点，同 fit：

| 端点 | |
| --- | --- |
| `POST /api/login` | `{"username", "password"}`，回答 `{"user", "csrf"}` 和会话的 cookie |
| `POST /api/logout` | 结束会话（`204`） |
| `GET /api/session` | `{"user", "csrf"}` |
| `GET /api/workbench` | 用户和提供的模块：`admin` |
| `GET /api/m/admin/users` | 已登录的用户、所有用户，以及是否有 store 保存它们 |
| `/api/m/admin/<路径>` | Admin API 的 `/<路径>`，用会话代替令牌，如 `GET /api/m/admin/config` |

程序保留 cookie，并在除 GET 和 HEAD 外的每个请求中把 `csrf` 放在 `X-Fit-Csrf` 中发送：

```bash
curl -c jar -H 'Content-Type: application/json' \
  -d '{"username": "admin", "password": "change me"}' http://127.0.0.1:2020/api/login
curl -b jar http://127.0.0.1:2020/api/m/admin/config
```

## 限制

| | |
| --- | --- |
| 连接 | `max_conns`（32）；超出时新连接在 accept 时被关闭 |
| 请求头 | 10 秒内读完 |
| 请求体 | `admin.max_config_bytes` 加 64 KiB，30 秒内读完；登录的请求体 4 KiB |
| 连接 | 两次请求之间保持 2 分钟；对没有有效会话的请求的回答会关闭连接 |
| 同时检查的密码 | 2 个，在一个单线程、16 个等待的池上 |
| 失败的登录 | 每个地址（IPv6 按 /64）15 分钟内 5 次，之后拒绝 15 分钟，一次只检查一个；最多记录 4,096 个地址 |

fit 还会在没有登录的连接打开 5 秒后关闭它，并在 `max_conns` 个连接都已打开时关闭最旧的未登录连接以腾出位置，为已登录的连接保留
四分之一；akit 等 akio 支持这一点（A232）。
