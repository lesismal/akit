# 035：Admin API 遮蔽秘密，akit 对暴露的秘密发出警告，以及 I80b 的其余局限

[English](035-admin-api-redacts-secrets.md) | [简体中文](035-admin-api-redacts-secrets.zh-CN.md)

状态：在 I80b 中做出（2026-11-26），等待维护者审阅（[待审阅的决定](../decisions-pending.zh-CN.md)，第 14 行）。

## 背景

I80 写了[威胁模型](../security/threat-model.zh-CN.md)及其已知的局限；I80b 关闭其中属于 akit 的各项。有几项需要做选择：

- **L2。** `GET /config`、`/versions/{n}` 和 `/config/effective` 按写入时的样子返回配置，包括内联的秘密（`password_hash`、
  路由设置的 `Authorization` 头、JWT 的 `secret`）。fit 自己的 Admin API 也是如此（[决定 002](002-config-compatibility.zh-CN.md)
  使其载荷成为 akit 的载荷），但 fit 的控制台、助手和集群管理从不显示秘密：凡 fit 的 `secretName` 认定为秘密的成员下的字符串，
  一律答 `[redacted]`（或 `[secret]`），修改送回该掩码时再把秘密放回（fit 的 `restoreSecrets`）。
- **L3、L4。** fit 读取秘密文件时不看权限，与 ACME、Consul、etcd、Redis、Docker 和 Kubernetes 通信时使用配置给出的任何传输。
  拒绝其中任何一种，都会拒绝 fit 能运行的配置。
- **L7。** akio 的 `Files` 跟随根目录下的符号链接，无论指向何处。fit 的 `static` 处理器有意如此（其包注释写明）；fit 没有
  反对它的选项。
- **L8。** akio-tls 能按 CRL（`ServerConfigBuilder::client_crls`）和装订的 OCSP（`client_ocsp_policy`）检查客户端证书，
  但 fit 的配置没有指明 CRL 的字段。
- **L9。** WASM 模块每次加载时从其 `path` 读取；fit 没有固定其摘要的字段。
- **L12。** akio 的请求头和请求体超时（10 秒、60 秒）不可配置；fit 的配置也没有面向客户端的请求头或请求体超时（其 `limits`
  没有；其 Admin API 和控制台在代码中设为 10 秒）。

## 决定

1. **Admin API 遮蔽内联的秘密。** 每次读取配置（`GET /config` 及其指针、`GET /versions/{n}`、`GET /config/effective`，
   以及基于它们的仪表盘）时，成员名表明其为密码、密钥、令牌或哈希的成员下的每个非空字符串都答 `[redacted]`，规则与 fit 的
   `secretName` 完全相同（不含 `*_file`、`*_env`、`*_ttl`、`max_*`、`min_*`），但 `rate_limit` 的 `key` 选择器（`ip`、
   `route`、`header:Name`）除外，推流目标的 `url` 也算秘密。没有秘密的配置逐字节按写入时的样子返回；有秘密的按 fit 写出编辑后
   配置的方式写出。`ETag` 仍是运行中配置的，所以对读到的内容可以照常用 `If-Match`。
2. **送回的 `[redacted]` 保留秘密。** `POST /load`、`PUT` 和 `PATCH` 对请求体中的每个 `[redacted]`，放回运行中配置在同一位置
   的秘密：按成员名、按命名对象列表中的 `name`、否则按下标匹配，与 fit 的 `restoreSecrets` 相同。其位置上没有秘密的
   `[redacted]` 以 400 拒绝（"... is [redacted], but no secret is kept there: write the secret itself"），什么也不改变。
   没有读回秘密的开关：需要秘密的人读配置文件。
3. **对暴露的秘密发出警告，而不是拒绝**（`akit_config::hardening`），在 `akit run` 启动时、`akit validate` 中，以及每次应用
   或检查修改时的 `warnings` 中：
   - `key_file`、`tls_key_file`、`token_file`、`password_file`、`hmac_secret_file` 或 provider 的 `kubeconfig` 所指的文件，
     其组或其他用户有任何权限时（每个文件一次），并给出要执行的 `chmod 600`；
   - 不在环回上的 `http://` ACME 目录；不在环回上、不经 TLS 访问的共享存储（其中保存私钥）：未设 `tls` 的 Redis，经
     `http://` 的 etcd 或 Consul；不在环回上经 `http://` 发送的 Consul 令牌或 etcd 密码；不在环回上经 `tcp://` 访问的 Docker API；
   - Kubernetes provider 的 API 服务器不在环回上且为 `http://` 或设了 `insecure-skip-tls-verify`，在找到集群时写入
     provider 的日志。
4. **证书存储收紧其目录。** fit 的文件存储使其创建的内容只有属主可读；akit 的还会把根目录及其下被写入的每个目录，在其组或其他
   用户有任何权限时收紧为 0700（尽力而为：不属于 akit 用户的目录保持原样），并在每次重命名后同步目录。
5. **与 fit 相同，继续跟随符号链接**（L7）。参考文档的 `static` 一节写明了这一点及其含义。拒绝指出根目录的链接的选项要先在
   fit 中设计（决定 002）：fit 的媒体库有 `follow_symlinks`，`static` 可以有同名字段。
6. **向 fit 提议，不在 akit 中实现**（决定 002）：L8 提议 `tls.client_auth.crl_files`（PEM 或 DER 的 CRL，像证书文件一样
   被监视），akit 会把它映射到 akio-tls 的 `client_crls` 上，无需修改 akio；L9 提议 `wasm` 中间件参数的 `module_sha256`，
   加载时 SHA-256 不符的模块被拒绝。
7. **L12 按现状关闭**：fit 的配置没有可读的字段，akio 的 10 秒和 60 秒就是 fit 自己的服务器所用的值。

## 后果

读取配置的运维人员、脚本或仪表盘不再看到内联的秘密，读出、编辑再送回的配置会保留它们。把字面字符串 `[redacted]` 写作秘密，
或依赖经 API 读回秘密的客户端，行为与 fit 的 Admin API 不同；兼容性页面写明了这一点。对秘密不加保护的配置会收到警告，这正是
目的；fit 能运行的配置一个也不拒绝。L8 和 L9 等待 fit。
