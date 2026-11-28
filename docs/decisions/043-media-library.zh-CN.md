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

## I145：频道

- **频道与 fit 的相同**：其状态、其条目的选择（列表、随机、节目表的时段切入和加入）、其时间线（每个包在解码时间到期时发送，下一个
  条目按其呈现时间放在上一个之后，同一轨道上的解码时间从不回退）、其 Admin API 和指标，名称为 akit 的（`akit_streaming_channel*`）。
- **计时器运行在 akit-media 的计时线程上**，所有频道共用一个线程，直到 akio 给处理器提供其引擎的计时器（A296）；频道的一次计时只
  持锁做几步并把包交给媒体中心，从不阻塞。影片和样本在媒体库的文件池上读取。
- **调用外部的工作在频道的锁释放后运行**：媒体库可能在调用者的线程上立即答复影片，答复会再次取锁；fit 为此另起一个 goroutine。
- **时区在角色应用时加载**，用 akit-observe 的读取器从系统的 zoneinfo 文件读取，与访问日志和 BitTorrent 的时间表相同；akit-config
  检查其名称的形式。时段在时钟调整时按书写的时刻落定，与 Go 的 `time.Date` 相同。
- **测试拨动时钟**（`clock::Clock`）：频道的节奏、节目表和重试都在测试的时间上检验，等待媒体库的文件池空闲，从不等待真实时间。
- **媒体库先于流媒体角色应用**，使频道在其第一个条目时就能找到其媒体库。

- **频道等待通知，从不靠计时器重试**：fit 稍后再试的地方，akit 的频道等待使其能继续的事件（akit 的计时器只用于与时间有关的逻辑，
  从不用于重试）：
  - 媒体库的首次扫描：媒体库在首个目录就位时或被移除时通知等待的频道（`Library::when_scanned`），fit 则每 200 ms 重试；
  - 因忙被拒的加载或读取，等待媒体库文件池或样本表有空位：媒体库的空位通知（`library::notice::Notice`）在每个任务离开池的队列、
    影片读取结束、池被替换或媒体库被移除时变化，频道在尝试前读取通知，等待其后的变化，因此不会错过；fit 则 200 ms 后重试；
  - 所有条目都失败或列表播完后，等待可播放的内容：扫描替换媒体库目录、重载或节目表的下一个时段，fit 则每 5 秒重试；
  - 流被其他发布者占用或媒体中心已满时，等待其流：媒体中心的 `on_unpublish` 经频道管理器通知，fit 则每 5 秒重试。不因发布者离开而
    释放的流位置（等待一个从未出现的流的播放器放弃）不会通知它；重载或 Admin API 的 start 会。

## I146：推送目标

- **SRT 推流是 akit 自己基于 akio-media 的 SRT 会话实现的**：用推流自己的处理器调用 `srt::dial`，处理器听到握手和关闭，其流像
  `srt::push` 那样封装为 MPEG-TS。akio-media 的 `srt::push` 没有重连，也不告诉其所有者任何事；推流在管理器的时钟上按 fit 的退避重试，
  这是配置所规定的。
- **RTMP 推流是 akio-media 的**，它同样不告诉其所有者任何事（akio API 请求 A298）：其重试是客户端的 `Reconnect`，使用 `retry_min` 和
  `retry_max`；对其流的监视（一个等待的订阅）在发布者回来时重新启动客户端，客户端在发布者离开后不会继续存在。没有计时器轮询客户端。其
  状态从客户端读取；在 A298 之前，其失败次数和字节数不统计，因凭据拒绝它的目标会结束它。
- **拨号在 akit-media 的池上运行**：它们解析主机名，会阻塞，而 `srt::dial` 会短暂等待其套接字；会话的回调也把工作交到那里，避开会话的锁
  （关闭会话要取这把锁）。
- **`rtmps://` 作为尚不支持而拒绝**，直到 akio-media 的 RTMP 客户端能通过 TLS 拨号（A297），而不是接受后永远连不上。
- **推送 URL 是机密**：错误和配置的 `Debug` 只显示其协议和主机，Admin API 显示其协议、主机和 app。

## I147：加密分片

- **密钥与 fit 逐字节一致**：同一媒体库名和 secret 给出相同的条目和频道密钥、相同的 IV（媒体序号）和相同的密钥 ID，因此从 fit 取得的
  密钥能解密 akit 的分片，反之亦然。点播分片的 ETag 带有密钥的 ID，其缓存键带有加密，因此新的 secret 产生新的分片和 ETag。
- **频道由 akio-media 的复用器加密**（`HlsEncryption`）：它加密分片和部分分片，并在其旁以 `key-<n>.key` 提供密钥，请求在到达 akio
  之前由处理器的 `auth` 授权。频道的两种格式是两个各有密钥的复用器，因此 MPEG-TS 复用器把其密钥命名为 `key-<n>.key?format=ts`，
  处理器按该查询参数路由密钥请求，就像路由播放列表一样。fit 把密钥命名为 `key-<session>-<n>.key` 并自己提供；两种方式下播放器都按
  播放列表的 `URI` 取用，因此名字不必相同。
- **会话属于复用器**：频道的密钥与 fit 一样从 `<stream>/<session>` 及其编号派生，重新启动的复用器（编号回到 0）取一个新的随机会话，
  因此其密钥和 IV 永不重复。
- **加密改变时生成新的复用器**：处理器保存频道的加密服务及其生成依据（媒体中心、媒体库、密钥 ID、轮换），任何一项改变后的第一个请求
  会生成新的服务，因此频道在使其加密的重载之后绝不以明文提供，也不会在改变密钥的重载之后仍用旧密钥；fit 出于同样的原因在下一个播放
  列表请求时结束其复用器。
- **没有 `auth` 的处理器不提供加密频道的密钥**（403），因为任何人都能拿到它；媒体库或密钥未加载的频道以 503 和 `Retry-After: 1`
  应答，绝不以明文提供。

## I148：视频网站

- **API 的应答体与 fit 相同**：片库和播放器页面是在钉住的版本复制的 fit 页面，因此 `api/library`、`api/library/<name>` 和
  `api/play` 逐个成员地给出 fit 的应答；互通测试在相同配置下与 fit 二进制比较它们。
- **`libraries` 的检查属于配置**：每个名字是否指向某个 `media_library`、是否只列出一次，需要配置中的媒体库，akit-config 在读完所有
  文件后才有它们，正如它检查 `vod` 的 `library`；其余检查（`vod` 的模板、没有 `hls` 的 `channels`）属于处理器，正如 fit 的属于其
  provision。无论哪种，配置都以 fit 的消息被拒绝。
- **指南是 `docs/guides/media-library.md`**，与 akit 的其他指南放在一起，而不是 fit 放置它的 `docs/media-library.md`；其中的配置与
  其他指南的一样由 `akit validate` 检查。

## I149：控制台的媒体页面

- **页面是 fit 的，端点是 akit-media 的**：控制台的 `gw.medialib` 模块随其他页面在钉住的版本复制；`akit_media::workbench` 应答它向
  Admin API 请求的内容，与 fit 的 `admin/media.go` 一样，成员和拒绝都相同。
- **端点读取 Admin API 保存的运行中配置**，在被请求时解析，并从引擎读取监听器绑定的端口，都在 crate 的池上；控制台的
  Content-Security-Policy 读取同样的内容，只在配置版本变化时重新读取。
- **控制台的用户和主机以 `Akit-Console-User` 和 `Akit-Console-Host` 传给它们**（fit 的 `Fit-Console-User` 和
  `Fit-Console-Host` 改名，决定 037），由控制台在其转发的请求上设置，从不取自浏览器；持有 Admin API token 的调用者可以自己设置。
- **上传的文件使用 akit 的名字**（`.akit-upload-<id>.part` 和 `.json`，整写文件用 `.akit-tmp-`；fit 的是 `.fit-upload-` 和
  `.fit-tmp-`），正如决定 037 对 akit 写入的所有文件名的要求。akit 不接管 fit 遗留的 `.fit-upload-*` 文件：037 只为配置和
  `fitstore` 存储保留 fit 的格式，此外不接受、读取或应答 fit 的任何名字；而上传只是客户端可以从其偏移续传的几分钟工作，不值得
  作为例外。扫描不查看以 '.' 开头的名字，因此不会动 fit 的文件；在 fit 上开始的上传要在 akit 上重新开始。
- **`PATCH` 的请求体被暂停而非阻塞等待**：fit 的处理器在写入一块时阻塞其 goroutine；akit 的暂停读取请求体（akio 的
  `BodyReads::hold`），在该块落盘后恢复，并收集已在进行的读取带来的数据，因此最多保留 512 KiB 加一次读取。被截断的请求体以已写入的
  部分结束写入，与 fit 的读取错误相同。
- **文件操作的错误与 fit 的含义相同，但没有 fit 的 `medialib:` 前缀**，与 akit 其他媒体库错误一致；其状态码与 fit 相同。
- **条目的使用情况从运行中的配置读取，在 crate 的池上进行**：媒体库的端点使用 Admin API 对配置的视图
  （`library::admin::serve_with`），与控制台的媒体端点相同；变更在该读取完成后才排入媒体库的池。
