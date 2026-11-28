# 媒体库与视频网站

[English](media-library.md) | [简体中文](media-library.zh-CN.md)

媒体库是 akit 为之编目的电影和剧集文件目录，供 `vod` 处理器以 HLS 点播提供，也供流媒体角色的频道当作直播播放。本指南把各部分
组合起来：媒体库、点播、会过期的播放地址、加密分片、频道，以及 akit 自己的片库和播放器页面。akit 读取 fit 关于这一切的配置，提供
fit 所提供的内容，因此网站可以带着配置在两者之间迁移（[决定 043](../decisions/043-media-library.zh-CN.md)）。
[配置参考](../reference/config.zh-CN.md)列出了每个字段。

## 媒体库

```fit
media_library films {
	roots /srv/media/films /srv/media/shows
	exclude **/extras/**
	scan_interval 30m
}
```

akit 按文件存储的原样提供其编码：H.264、H.265 和 AV1 视频，AAC、Opus 和 MP3 音频，位于非分片的 MP4 文件中。版本（例如
1080p 和 720p 的同一部影片）是事先用 ffmpeg 制作的各自独立的文件；akit 从不转码。文件按名字归为条目：`bbb.mp4` 和
`bbb.720p.mp4` 是条目 `bbb` 的版本，`bbb.audio-fr.m4a` 是其法语音频，`bbb.en.srt` 是字幕，`bbb.jpg` 是海报，`bbb.json`
是补充说明文件，给出名字无法表达的内容。条目的 ID 是其在根目录下的路径，去掉标签和扩展名：`films/bbb.720p.mp4` 的 ID 是
`films/bbb`。

媒体库在创建时、每隔 `scan_interval`、重载改变它时以及 Admin API 的 `POST /media/library/<name>/rescan` 时扫描；
`GET /media/library` 和 `GET /media/library/<name>` 读取目录。文件在媒体库的文件池上探测和读取，从不在 I/O 线程上。

## 点播

```fit
listener web :8080
media_library films { roots /srv/media/films }

route films {
	match { path /vod/* }
	handler vod {
		library films
		segment_duration 4s
		cache_control "public, max-age=86400"
	}
}
```

`/vod/films/films/bbb/index.m3u8` 播放条目 `films/bbb`：其多码率播放列表，由 fMP4 分片组成（MPEG-TS 为 `index.ts.m3u8`），
在 `segment_duration` 附近的关键帧处切分，并带有条目的音频和字幕。`auth` 指定每个请求都必须满足的 `media_auth` 策略，需要对
`vod/<library>/<id>` 有 `play` 权限。

## 会过期的播放地址

由网站自己的 API 决定谁能观看，akit 负责检查。设置策略的 `url_signing` 后，网站向 `media_tokens` 申请为某个观众、其地址和一个
会话签名、在一段时间内有效的地址；`vod`、`hls` 和 `httpflv` 处理器在每个请求上检查它，用它取得的播放列表交给播放器一个授权，
使其能播放到结束。`max_sessions` 限制一个观众同时拥有的播放器数。在相同的密钥、密钥环和策略名下，fit 签名的地址能在 akit 上
播放，akit 签名的也能在 fit 上播放。

```fit
media_auth site {
	secret_file site-secret
	url_signing { ttl 2m; play_grace 30m; max_sessions 2; on_limit end_oldest }
}
```

## 加密分片

设置 `encrypt` 时，`vod` 用 AES-128（`METHOD=AES-128`）加密每个分片，密钥是从媒体库的 `secret` 或 `secret_file` 派生的条目
密钥，在媒体播放列表中以 `aes.key` 给出，只提供给 `auth` 允许播放该条目的请求；频道的 `encrypt` 对其 HLS 做同样的事，每
`key_rotation` 个分片换一把新密钥，由设置了 `auth` 的 `hls` 处理器提供。密钥与 fit 逐字节一致：同一 secret 在 fit 和 akit 上
得到相同的密钥。更换 secret 会改变所有密钥和分片的 ETag，因此要清除 CDN 保存的内容。

```fit
media_library films {
	roots /srv/media/films
	secret_file films-secret
}
```

## 频道

频道把媒体库的条目当作直播流播放，按顺序、随机、循环或按节目表；流媒体角色的每种出口都在其 auth 下播放它，`push` 把它转推到
RTMP 或 SRT 服务器。Admin API 在 `/streaming/channels` 列出、启动、停止和跳过频道。

```fit
media_library films { roots /srv/media/films }

streaming {
	channels {
		name tv/films
		library films
		items films/testcard films/bars
	}
}
```

## 视频网站

把一切组合起来，正如 fit-examples 的 `video-site`：媒体库中的影片以点播和频道两种方式播放，观众由网站自己的 API 放行，通过会
过期的地址观看；akit 自己的片库和播放器页面给工作人员使用。

```fit
listener web :8080

media_library films { roots video-site/media }

media_auth site {
	secret_file site-secret
	users staff { password_hash $2a$10$gkDJyGagxp1q7R24F0rSjulsgZOFL55Zm6JK2RcxTCLDk23szhZA2; play * }
	url_signing { ttl 2m; play_grace 30m; max_sessions 2; on_limit end_oldest }
}

streaming {
	channels { name tv/films; library films; items films/testcard films/bars }
}

upstream site-api { addresses 127.0.0.1:8090 }

route vod { match { path /vod/* }; handler vod { library films; auth site } }
route channels { match { path /hls/* }; handler hls { prefix /hls/; auth site } }
route tokens {
	match { path /media-tokens /media-tokens/*; remote_ip 127.0.0.1/32 ::1/128 }
	middleware basic_auth { users { username site-api; password_hash $2a$10$gkDJyGagxp1q7R24F0rSjulsgZOFL55Zm6JK2RcxTCLDk23szhZA2 } }
	handler media_tokens { auth site; url_prefixes /vod/films/ /hls/tv/; max_ttl 1h }
}
route api { match { path /api/* }; handler proxy { upstream site-api } }
route staff {
	match { path /media /media/* }
	handler media_pages { auth site; login true; libraries films; channels true; hls /hls/{stream}/index.m3u8 }
}
```

**网站的 API** 在自己的数据库中保存观众，并向 `media_tokens`（只接受本机请求，位于 `basic_auth` 之后）为每个观众申请某个
影片的地址，由其页面交给 hls.js。`media_pages` 在 `<prefix>/vendor/hls.min.js` 提供 hls.js，供网站的任何页面加载。

**akit 给工作人员的页面。**设置了 `libraries` 和 `channels` 的 `media_pages` 在策略登录之后提供片库页面 `<prefix>/library`
和播放器页面 `<prefix>/watch`：片库列出该用户可以播放的条目和频道（对 `vod/<library>/<id>` 或对频道有 `play` 权限），带有海报，
可搜索和分页；播放器用 hls.js 播放其中之一，地址由页面为该用户、其地址和它自己的会话签名（`api/play`），策略不签名时则用普通
地址，由登录的 cookie 放行。`vod` 是条目播放列表的地址，未设置时为 `/vod/{library}/{id}/index.m3u8`，`hls` 是频道的。
