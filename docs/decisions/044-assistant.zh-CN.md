# 044：助手：控制台中的 fit 助手、fit 的线上协议、akit 的名字

[English](044-assistant.md) | [简体中文](044-assistant.zh-CN.md)

状态：在本路线的计划中采纳（2026-11-28），待审阅（[待审阅的决定](../decisions-pending.zh-CN.md)第 98–102 行）；随本路线的
迭代（I160–I169）落地而扩充。

## 背景

fit 的 I124–I130 和 I152–I154 为 fit 的工作台加入了助手（[fit 的决定
031](https://github.com/lesismal/fit/blob/main/docs/decisions/031-assistant.md)）：每个页面旁的对话面板，经进程内调用的 Admin API
读取运行中的 fit 在做什么、加以解释，并起草改动：要么写进打开的页面成为未保存的草稿，要么成为由人点击并再次确认后应用的提案。它调用
其设置所指定的模型 API；它把设置、提供方的密钥、用户的会话、技能、审计日志和 MCP 令牌保存在配置旁加密存储的分区中；一个 MCP
服务器让外部代理使用它的工具。fit 工作台的页面（其中包括模块 `ai`）已经在 akit 中：I91 复制了它们，决定 037 改掉了其中关于 fit
的名字。akit 的集群管理端（I132–I139）对用户的助手级别应答为空，因为 fit 的 I153 留给了本路线。

akit 如何跟随取决于三件事：助手放在 akit 的哪个 crate 中，保留 fit 的哪些线上协议，以及它的模型调用如何满足 akit 关于阻塞、上限和
重试的规则。

## 决定

1. **路线是 fit 的，分十个迭代**（I160–I169，开发计划的"AI 助手"），按 fit 的顺序，只有两处改动：fit 的 I124 拆为设置（I160）
   和模型客户端（I161）；fit 的 I128 和 I129 合并（I165），因为 fit 的 I128 是页面自身的工作，I91 已经复制。fit 的 I153，
   跨集群的助手，放在最后（I169），因为它需要集群路线的 I139，其指标、日志和证书还需要 I139c。
2. **一个独立的 crate `akit-assistant`，由控制台创建。** 如 fit 的控制台创建其助手（`admin/console.go`），设置了
   `admin.assistant` 时由 akit-admin 的控制台创建它，并作为控制台模块 `ai` 在配置本身的模块和角色的模块旁提供（fit 的
   `ModuleID`）；akit-admin 依赖 akit-assistant，从不反过来。助手经 akit-admin 给它的读取器在进程内读取 Admin API，即工作台
   页面所调用的那些端点，除了经由人的应用（I163）之外不做任何改动。集群管理端经由一个只含普通类型的接口（fit 的
   `assistant.Cluster`）与它衔接，两个 crate 互不导入（I169）。
3. **fit 的线上协议**：模块的 API（`/api/m/ai/...`，其 JSON 成员和错误）、对话的 Server-Sent Events、存储的分区 `assistant`
   （`{"settings", "keys"}`）及其同级分区（`assistant.chats.<hash>`、`assistant.skills`、`assistant.audit`、
   `assistant.mcp`）、技能的格式，以及 MCP 协议及其修订版，都是 fit 的，使页面不改即可运行，存储连同其助手在 akit 与 fit 之间
   迁移（决定 036 第 6 条关于 `fitstore` 格式），任一方的 MCP 客户端都能连上两者。写着 fit 的名字改为 akit（决定 037）：MCP
   服务器的名字、其资源的 scheme `akit://`（fit 的 `fit://`）、系统提示和技能中称呼 akit；配置的字段是 fit 的。
4. **是模型 API 的客户端，从不是模型。** fit 的八种 kind（`anthropic`、`openai`、`openai_responses`、`gemini`、
   `azure_openai`、`ollama`、`openai_compatible`、`llm_proxy`），每种的请求和流式应答都按 fit 的方式构造和解析。调用在
   akio-http 的异步客户端上（规则 1）、在监听器的 engine 上发出，如 fit 在 fib 上那样；没有任何东西在 I/O 线程上等待，工具在助手
   自己的线程池上运行，读写存储也在其上。
5. **有上限，背压相连。** 每个缓冲都按 fit 的上限：模型流的一行或一个事件 1 MiB、一个应答 `limits.max_stream_bytes`、64 个
   工具调用、错误消息 8 KiB、工具结果 `max_tool_result_bytes`，会话、技能、审计条目和令牌按各自的上限。对话发往浏览器的流与模型的
   流相连（规则 8，待定决定第 102 行）：浏览器未取走的字节超过水位时暂停模型的应答，待其排空后恢复；fit 则不等待地继续写。
6. **不做基于定时器的重试。** 模型调用只会立即重试，只在尚未取得任何应答时，且只在连接失败或关闭时（待定决定第 100 行）；408、
   429 或 5xx 的应答使调用失败，并说明 API 的要求（其 `Retry-After`），由人再次发送。fit 会等待 `Retry-After`（最多 10 s）
   后重试。
7. **秘密从不外泄**，如 fit：密钥保存在存储中，只写一次、遮掩显示（16 个字符及以上的密钥显示最后四个字符），从不应答、记录日志、
   导出或放进错误中；密钥只经 https 或发往回环主机；每个工具结果、差异和草稿都按 fit 的方式脱敏（akit-admin 的 `redact`，它已
   遵循 fit 的 `assistant/redact.go`），任何工具都不读取的成员被略去。
8. **默认关闭。** 没有 `admin.assistant`：没有模块 `ai`、没有面板、没有任何外发调用。与整个 `admin` 一样，它在 akit 启动时生效；
   "设置"页对设置的更改立即生效，保存在存储中。
9. **测试从不连接真实模型。** 每个测试都与端口 0 上的模型 API 替身（移植自 fit 的 `fakeapi`）通信，或与 akit 自己的 `llm.proxy`
   及其上游为这种替身的 node 通信；没有测试需要密钥或互联网。对真实模型的评测（fit 的 `make assistant-eval`，I165）只在有人在
   环境中指定模型时运行。

## 后果

- akit-admin 依赖 akit-assistant，这把模型客户端带入了 akit-admin 的依赖；不带控制台助手构建的二进制并不因此更小。
- 带助手的 akit 写入的存储是版本 3（分区），与 fit 相同；早于分区的 akit 或 fit 无法打开它。
- 在模型 API 限流时发送消息的用户会得到拒绝及 API 要求等待的时长，而不是在静默等待之后得到应答。
- 慢的浏览器会放慢它所读取的模型流，而不是增长 akit 的内存；在 fit 的 60 s 内什么都不取的浏览器仍会结束其对话。
