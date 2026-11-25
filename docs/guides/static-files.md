# Static files and PHP

[English](static-files.md) | [简体中文](static-files.zh-CN.md)

The `static` handler serves a directory; the `fastcgi` handler runs PHP, or any FastCGI
application, behind PHP-FPM. Both look at the disk on akio's file pool, never on the threads that
serve connections, so a slow disk slows the requests that need it and nothing else. The
[reference](../reference/config.md#static) has every field.

## A site

```fit
listener web :8080

route {
	handler static {
		root public
		index index.html
	}
}
```

`static` answers `GET` and `HEAD` with `ETag` and `Last-Modified`, conditional requests (304) and
ranges (206), and a content type by the file's extension. Files up to `max_file_bytes` are kept in
memory, up to `max_cache_bytes` in all, and checked against the disk at most every
`check_interval`. A path with a segment that starts with a dot (`/.git/config`, `/.env`) is answered
404, but under `/.well-known/`; `dotfiles allow` serves them.

## A single-page app

`try_files` tries paths in turn and serves the first that exists: here the file asked for, then a
directory's index, then the app's `index.html`, so that the app's own routes load the app.

```fit
listener web :8080

route {
	handler static {
		root public
		try_files {path} {path}/ /index.html
	}
}
```

## Compression

`precompressed` serves a file's twin compressed ahead of time (`app.js.br`, `app.js.gz`) to a
client that takes it; `compress` keeps compressed copies of the files held in memory, made once on
the file pool. The `compress` middleware compresses whatever else a route answers, as it is
written.

```fit
listener web :8080

route {
	handler static {
		root public
		precompressed br gzip
		compress zstd br gzip
	}
}
```

## Under a prefix, and listings

`strip_prefix` serves a directory under a path of its own; `browse` lists a directory that has no
index.

```fit
listener web :8080

route {
	match { path /downloads/* }
	handler static {
		root public
		strip_prefix /downloads
		browse true
	}
}
```

## PHP

A route for scripts, through `fastcgi`, beside one for the rest of the files: `try_files` sends a
path that is no script of its own to the front controller, as most PHP frameworks want. PHP's
parameters (`SCRIPT_FILENAME`, `PATH_INFO` and the rest) are set from `root` and `split_path`; `env`
adds more. The connections to PHP-FPM are a bounded pool (`max_conns`, `max_idle_conns`), and bodies
are streamed both ways.

```fit
listener web :8080

route php {
	match { path_regexp (?i)\.php$ }
	handler fastcgi {
		address 127.0.0.1:9000
		root /var/www/app/public
	}
}

route app {
	handler fastcgi {
		address unix//run/php/php-fpm.sock
		root /var/www/app/public
		try_files {path} {path}/index.php /index.php
	}
}
```

Write the script route as `(?i)\.php$`: on a file system that folds case, `/x.PHP` opens `x.php`.
`static` never serves a file whose extension a `fastcgi` handler's `split_path` names, so a script's
source is never served as text from a root the two share.
