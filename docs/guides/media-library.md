# Media library and a video site

[English](media-library.md) | [简体中文](media-library.zh-CN.md)

A media library is directories of film and episode files that akit catalogues, for the `vod`
handler that serves them as video on demand over HLS and the streaming role's channels that play
them as live streams. This guide puts the parts together: the library, video on demand, playback
URLs that expire, encrypted segments, channels, and akit's own library and player pages. akit reads
fit's config for all of it and serves what fit serves, so a site moves from one to the other with
its config ([decision 043](../decisions/043-media-library.md)). The
[config reference](../reference/config.md) lists every field.

## The library

```fit
media_library films {
	roots /srv/media/films /srv/media/shows
	exclude **/extras/**
	scan_interval 30m
}
```

akit serves the codecs a file holds as they are stored: H.264, H.265 and AV1 video, AAC, Opus and
MP3 audio, in MP4 files that are not fragmented. Renditions, a film at 1080p and at 720p, are files
of their own made beforehand with ffmpeg; akit never transcodes. Files are grouped into items by
their names: `bbb.mp4` and `bbb.720p.mp4` are renditions of the item `bbb`, `bbb.audio-fr.m4a` its
French audio, `bbb.en.srt` a subtitle, `bbb.jpg` its poster, and `bbb.json` a sidecar that names
what the names cannot. An item's ID is its path under its root without its label and extension:
`films/bbb` for `films/bbb.720p.mp4`.

A library is scanned when it is made, every `scan_interval`, when a reload changes it, and on the
Admin API's `POST /media/library/<name>/rescan`; `GET /media/library` and
`GET /media/library/<name>` read the catalog. Files are probed and read on the library's file pool,
never on an I/O thread.

## The console's pages

With `admin.console`, the console's **Media library** pages browse each library's items, play a
preview through a URL signed for the user logged in, edit the channels on a timeline, issue test
URLs, list and end playback sessions, rotate keys kept in files, chart what is served, and upload files into a library's `upload_dir`,
resumable after a cut connection (bounded by `max_upload_bytes`, `max_uploads`, `min_free_bytes`
and `upload_expiry`), and rename, move and delete items and edit their sidecars. Behind
them, the Admin API's `POST /media/preview`, `GET /media/auth` (with `/sessions`,
`/sessions/end`, `/rotate` and `/urls` of a policy), `GET /media/stats`, and a library's
`/media/library/<name>/uploads`, whose `PATCH` writes a piece at `Upload-Offset`, as tus does,
and its `/rename`, `/move`, `/delete` and `/sidecar`: an item that a channel names or plays, or a
viewer plays, is changed only with `force`, as the pages ask before they send it.

## Video on demand

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

`/vod/films/films/bbb/index.m3u8` plays the item `films/bbb`: its multivariant playlist, of fMP4
segments (`index.ts.m3u8` for MPEG-TS), cut at keyframes near `segment_duration`, with the item's
audio and subtitles. `auth` names a `media_auth` policy every request must satisfy, with `play` on
`vod/<library>/<id>`.

## Playback URLs that expire

The site's own API decides who watches; akit checks. With a policy's `url_signing`, the site asks
`media_tokens` for a URL signed for one viewer, its address and a session, good for a while; the
`vod`, `hls` and `httpflv` handlers check it on every request, and a playlist fetched with it hands
the player a grant to play on to the end. `max_sessions` caps the players a viewer has at once.
URLs signed by fit play on akit and those akit signs on fit, given the same secret, keys and policy
name.

```fit
media_auth site {
	secret_file site-secret
	url_signing { ttl 2m; play_grace 30m; max_sessions 2; on_limit end_oldest }
}
```

## Encrypted segments

With `encrypt`, `vod` encrypts every segment with AES-128 (`METHOD=AES-128`) under a key of the item
derived from the library's `secret` or `secret_file`, names it as `aes.key` in the media playlists,
and serves it only to a request its `auth` lets play the item; a channel's `encrypt` does the same
for its HLS, a new key every `key_rotation` segments, served by an `hls` handler with `auth`. The
keys are fit's byte for byte: the same secret gives the same keys on fit and on akit. Changing the
secret changes every key and the segments' ETags, so purge what a CDN kept.

```fit
media_library films {
	roots /srv/media/films
	secret_file films-secret
}
```

## Channels

A channel plays a library's items as a live stream, in order, shuffled, looped or on a schedule;
every egress of the streaming role plays it under its auth, and `push` relays it to RTMP or SRT
servers. The Admin API lists, starts, stops and skips channels at `/streaming/channels`.

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

## A video site

All of it together, as fit-examples' `video-site` is: films in a library played on demand and as a
channel, by the viewers the site's own API lets in, through URLs that expire; akit's own library and
player pages for the staff.

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

**The site's API** keeps the viewers, in its own database, and asks `media_tokens` (from the
loopback alone, behind `basic_auth`) for a URL of a title for each viewer, which its page hands
hls.js. `media_pages` serves hls.js at `<prefix>/vendor/hls.min.js` for any page of the site to load.

**akit's own pages for the staff.** `media_pages` with `libraries` and `channels` serves a library
page, `<prefix>/library`, and a player page, `<prefix>/watch`, behind the policy's login: the
library lists the items and channels the user may play (`play` on `vod/<library>/<id>`, or on the
channel), with their posters, searched and paged; the player plays one with hls.js through a URL
the pages sign for that user, its address and a session of its own (`api/play`), or the plain URL,
which the login's cookie lets in, when the policy signs none. `vod` is the URL of an item's playlist,
`/vod/{library}/{id}/index.m3u8` unless set, and `hls` a channel's.
