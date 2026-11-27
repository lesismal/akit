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
