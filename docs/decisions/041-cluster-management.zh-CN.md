# 041：集群管理：fit 的管理端、fit 与成员间的线上协议、akit 的名字

[English](041-cluster-management.md) | [简体中文](041-cluster-management.zh-CN.md)

状态：在路线计划中接受（2026-11-27），等待审阅（[等待审阅的决定](../decisions-pending.zh-CN.md)，第 70–74 行）；路线的
迭代（I130–I139c）完成时陆续补充。

## 背景

fit 的 I140–I150 让设置了 `admin.cluster` 的 fit 成为其他 fit（其成员）的管理端
（[fit 的决定 033](https://github.com/lesismal/fit/blob/main/docs/decisions/033-cluster-management.zh-CN.md)）。管理端把成员、
凭据、分组、模板、发布和审计日志保存在其存储的 `cluster` 分区，轮询每个成员的 `GET /node`、`/status` 和
`/config/check`，并通过成员自己的 API 修改成员：用户名和密码登录的控制台，或带 token 的 Admin API。无法直接访问的成员
主动外连到管理端（I149）。工作台的集群页面画出这一切；I91 已把这些页面随 fit 的其他页面复制进 akit（按
[决定 037](037-akit-names.zh-CN.md) 改名）。

有两件事决定 akit 如何跟随：保留 fit 管理端与成员之间协议的哪些部分，以及其中哪些名字属于 akit。

## 决定

1. **路线是 fit 的，分十二次迭代**（I130–I139c，开发计划的"集群管理"），按 fit 的顺序；fit 的 I153（跨集群的助手）随
   需要它的 AI 助手路线进行。管理端是一个独立的 crate `akit-cluster`，由 `admin.console` 作为控制台模块 `cluster` 提供。
2. **成员的回答是 fit 的协议**：路径（`/node`、`/status`、`/config/check`、`/schema`、`/api/login`、`/api/logout`、
   `/api/node`、`/api/m/admin/...`）、JSON 成员、能力名（`config.dry_run`、`status.requests` 等）、问题类型、存储的
   `node` 和 `cluster` 分区以及集群文档的结构。它们是协议标识，正如决定 038 第 3 点保留隧道的标识。以 fit 自己命名的成员
   按决定 037 处处改名，因为页面就是这样读取的：`GET /node` 回答 `akit_version`，而不是 `fit_version`。页面与之并列显示的
   `fib_version` 和 `go_version`（fit 的引擎和工具链）填 akit 的：`akio <提交>` 和空字符串（akit 的构建不记录编译器版本）。
3. **管理端以同样方式对待 fit 和 akit 成员。** akit 的管理端把会话的 CSRF 令牌同时放在 `X-Akit-Csrf` 和 fit 的
   `X-Fit-Csrf` 中，两边各读自己的；读到 fit 成员的 `fit_version` 时把版本记为 `fit <版本>`，让页面显示成员是哪一种。fit
   的管理端通过 akit 成员的 Admin API（`admin_api`，token）访问它：akit 的控制台只读 `X-Akit-Csrf`（决定 037：不留别名），
   所以登录到 akit 控制台的 fit 管理端能读不能写。
4. **存储可以共用**：`cluster` 文档是 fit 的，读取版本 1 到 3，没有角色时写版本 2，有角色时写版本 3，与 fit 相同；akit
   管理端尚未用到的部分（在其迭代之前的分组、模板、发布、链接、角色）按读到的样子保留并原样写回，使存储在 akit 与 fit 管理端
   之间移动时不丢失它们。更新版本的文档只读显示，与 fit 相同。
5. **akit 显示的名字是 akit 的**（决定 037）：配置旁保存节点 ID 的文件为 `akit.node-id`；注册 token 和凭据为 `akite1.` 和
   `akitc1.`（fit 的 `fite1.` 和 `fitc1.`）；管理端的 `User-Agent` 为 `akit-cluster-manager`。token 在 akit 与 fit 之间移动
   时改写其前缀，正如决定 038 第 4 点对链路 token 所说。

## 备选方案

- **线上也用 akit 的名字**（只用 `akit_version`、只用 `X-Akit-Csrf`）：一种管理端就无法管理另一种成员。管理端一侧拒绝；
  成员控制台一侧保留，因为决定 037 已改名其 CSRF 头。
- **akit 成员同时回答 fit 的 `fit_version`**：这是别名，决定 037 拒绝；fit 管理端对 akit 成员不显示版本。

## 后果

- akit 和 fit 成员可以共用任一种管理端：akit 管理端以控制台或 token 访问两者；fit 管理端以 token 访问 akit 成员。
- fit 管理端的节点切换器把 akit 成员视为另一种 schema（其摘要来自 akit 改名后的 schema）：在那里只读，正如 fit 的 I143 对
  任何它不认识的 schema 的处理。
