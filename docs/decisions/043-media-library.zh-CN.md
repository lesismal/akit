# 043：媒体库：fit 的成员、akio-media 的 MP4 读取器和 HLS、fit 的格式

[English](043-media-library.md) | [简体中文](043-media-library.zh-CN.md)

状态：在 I140 中接受（2026-11-27），等待审阅（[等待审阅的决定](../decisions-pending.zh-CN.md)，第 86–90 行）；随媒体库与频道
路线（I140–I149）各迭代落地而扩充。

## 背景

fit 的 I131–I139（[fit 的决定 032](https://github.com/lesismal/fit/blob/main/docs/decisions/032-media-library.zh-CN.md)）
新增了媒体库：把若干目录中的 MP4 文件扫描成目录（`media_library`），不转码地以 HLS 点播（`vod`），作为直播频道播放进流媒体
中心并推送到外部服务器（`streaming.channels`、`push`），放在会过期的播放 URL 之后（`media_auth` 的 `url_signing`，基于 I119
的 `keys` 和 `media_tokens`），支持 AES-128 分片、视频网站页面和工作台页面。fit 为此向 fib 请求了非分片 MP4 文件的读取器
（fib 的 A64）以及其直播 HLS 复用器的加密（fib 的 A69）。

akio-media 已经具备两者：`akio_media::mp4`（akio ADR-097）是这类文件的 sans-io 读取器（在文件开头或末尾找到 `moov`，读出每条
轨道的编解码器和紧凑的样本索引，像播放器那样应用编辑列表，每个计数都按上限检查），并带有一个规划器，在关键帧处把文件切成
HLS 分片，再把分片复用为 fMP4 或 MPEG-TS；akio-media 的 `Hls` 支持 AES-128 和 SAMPLE-AES，密钥来自 `KeyProvider` 并可轮换；
akio-crypto 有带 PKCS #7 填充的 AES-128-CBC。流媒体路线（决定 040）把 `media_pages` 的 `libraries`、`vod` 和 `channels`，策略
的 `keys` 和 `url_signing`，以及角色的 `channels` 和 `push` 作为“尚不支持”拒绝，留给本路线。

## 决定

1. **各成员按 fit 的方式读取**，每个在运行它的迭代中读取，此前以“尚不支持”拒绝；首先是 `media_library`（I140），字段、默认值、
   上限和检查与 fit 相同，位于 `akit-config` 的 `medialib` 模块。
2. **MP4 读取器用 akio-media 的。** fit 的 I131 在 akio 中已经完成：akit 用 `akio_media::mp4`（`Locator`、`Movie`、`Plan`、
   `Segment`）读取文件；fit 的读取器做而 akio 的不做的部分，按名字在 akio 需求 A295–A300 中向 akio 请求（规则 21），在此期间
   采用可靠的临时方案，或拒绝该字段。
3. **一个 crate，`akit-media`**，在流媒体角色和策略旁容纳媒体库、`vod` 和频道（没有新类别的依赖：固定提交上的 akio crate）。
4. **名字用 akit 的，格式用 fit 的**（决定 037）：指标 `akit_media_library_*`、上传的临时文件 `.akit-upload-*`；但凡属格式的
   部分逐字节保持 fit 的，如同 token（决定 040）：由路径生成的条目 ID、播放 URL 的字段和签名以及派生其密钥的标签、从媒体库
   密钥派生 AES-128 密钥的方式，使 fit 签发的 URL 能在 akit 上播放，且在密钥相同时双方派生的密钥一致。
5. **与 fit 一样不阻塞 I/O 线程**：媒体库在自己的线程上扫描；服务时在媒体库自己的有界文件池（`file_workers`、`file_queue`、
   `file_timeout`）上读取文件，期间请求被挂起（`Context::suspend`）；目录、保留的样本表和构建出的分片各自有字节上限。

## 备选方案

- **akit 自己的 MP4 读取器**，从 fib 的 `media/mp4` 移植：同一技术栈中会有同一格式的两个读取器，而 akio 的读取器经过模糊测试，
  且承载 akio-media 复用器所接受的编解码器。拒绝（规则 21）。
- **在格式上使用 akit 的名字**（URL 签名和密钥用另一个标签）：fit 和 akit 的 URL 与密钥将互不接受，只为了没人看得见的名字。
  拒绝，与 token 相同。

## 影响

- 同一个媒体库由 fit 和 akit 以相同方式扫描、播放和签名；在密钥相同时，网站可以在两者间迁移，或在一个 API 后同时运行两者。
- 当 akio-media 的读取器或规划器的结果与 fib 不同（分片在哪里切、探测读取什么）时，akit 按 akio 的结果回答；每处差异在遇到它
  的迭代中记录，在 fit 的行为有意义时向 akio 请求。

## I141：目录

- **在媒体库自己的线程上扫描。** 媒体库在自己的线程上扫描，在被请求时、重载改变扫描内容时以及每个 `scan_interval` 被唤醒
  （条件变量的定时等待：其逻辑关乎时间）。其探测在扫描自己的至多 `max_probes` 个作用域线程上运行，而不像 fit 那样在文件池上，
  使扫描永远不必为重试被文件池拒绝的任务而睡眠，文件池的工作线程留给正在提供的文件（待审决定第 88 行）。
- **探测整个解析 `moov`**，用 `Movie::parse`，只保留其描述；提供文件时再次解析影片。fib 读取 `moov` 时不读样本表；akio 需求
  A295 请求这一点（待审决定第 88 行）。
- **每个文件保留一个影片**，而不是每条轨道一个表：`Movie::parse` 一次构建所有轨道的索引。缓存与 fit 一样受 `max_table_bytes`
  限制，按探测解析时得到的索引字节数接纳影片。
- **文件池的线程随工作到来而启动**，至多 `file_workers` 个，而不是一次全部启动。
- **Admin API 的 `/media/library`** 由 `akit-media` 提供，由程序注册为 Admin API 的端点（`akit_admin::Options::endpoints`），使
  akit-admin 不依赖媒体 crate。
- **akio 读取文件与 fib 不同之处**，目录随 akio（待审决定第 89 行）。

## I142：点播

- **分片由 akio-media 的 `mp4::Plan` 切分**，由其 fMP4 和 MPEG-TS 复用器复用，来源是媒体库保留的影片；播放列表承载文件中第一条
  所服务编解码器的视频轨和第一条音频轨（与 fit 的选择相同），其他轨道以未知轨道交给复用器，使其略过。`Plan` 与 fit 的规划器
  切分不同之处，akit 随 `Plan`（待审决定第 90 行）。
- **一个时钟，与 fit 相同**：移动每个包的时间，使文件的时间 0 在两种容器中都位于 1.4 秒，从而用一个 `X-TIMESTAMP-MAP` 定位字幕，
  与 fit 相同。
- **命名、多码率播放列表和字幕与 fit 相同**，按 fit 的写法写出；H.264 编解码器字符串用 fib 的小写十六进制。
- **文件本身和海报经由该条目根目录的 akio-http `Files` 提供**，带范围、条件请求和 `sendfile`。
- **等待文件池的请求被挂起**（`Context::suspend`），由文件池回答；在 akio 的 A296 之前，超过 `timeout` 时由 akit-media 自己的计时
  线程回答 503。

## I143：媒体 token

- **密钥环和 `media_tokens` 是 fit 的 I119**，由于签名 URL 建立在其上，本路线承担了它。token 用密钥环的第一个密钥或批次指定的
  密钥签名，并对每个密钥校验；token 中不含密钥名，与 fit 相同。
- **路由必须认证其调用方**：akit-config 拒绝没有 `jwt`、`basic_auth`、`forward_auth` 或不带 `optional` 的 `client_cert` 的
  `media_tokens` 路由，与 fit 的 graph 相同；也拒绝缺少撤销所需轮换的密钥文件的策略。
- **批次的请求体由 akio-http 在监听器的上限内整个读取**，再按 `max_body_bytes` 检查（413），而 fit 以流方式读取；它至多 1 MiB。
- **撤销**把每个新密钥（32 个随机字节，base64url）写入密钥文件旁的临时文件，权限 0600，同步后重命名覆盖，在处理器自己的一个
  线程上进行，队列长度为一；请求被挂起并从那里回答，在 akio 的 A296 之前超过 `revoke_timeout` 时由 akit-media 的计时线程回答 504。

## I144：会过期的播放 URL

- **URL 和授权与 fit 逐字节相同**：密钥为 `HMAC-SHA256(签名密钥, "fit media url\0")`，MAC 覆盖 `fit-url-1`、资源和各字段，
  取前 16 字节的 base64url，查询的顺序和转义也相同。同一密钥或密钥环、同一策略名下，fit 签发的 URL 能在 akit 上播放，akit 签发的
  也能在 fit 上播放；一个单元测试固定了 fit 的 `SignURL` 给出的查询。授权所在的 cookie 为 `akit_media_grant`（决定 037），因此
  保存 cookie 的播放器不会在同一主机上把一个网关的授权带到另一个网关。
- **检查在 I/O 线程上进行，没有回调**，与 fit 相同：MAC、过期时间、地址、`Referer` 和 `Origin`，然后在分片锁下处理会话条目。
  它会做少量分配（在目标被脱敏前复制的查询、授权的字段）；fit 的不分配。
- **测试指定时间**而不是拨动时钟：策略的检查和会话把时间作为参数（`let_in`、`pass_on_at`、`sessions_at`），处理器从时钟给出；
  处理器的测试签发已经过期或一小时内有效的 URL。
- **传递授权或 token 的播放列表属于观看者自己**（`private, no-store`）；开启 `cookie` 时，缓存保存的媒体播放列表发送时带上授权的
  cookie，与 fit 相同。
- **Admin API 的会话和拒绝计数**（fit 的 `admin/media.go`）随控制台页面而来（I149）；策略现在已有它们（`sessions`、
  `end_session`、`url_refusals`）。
