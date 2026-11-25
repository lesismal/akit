# 静态文件与 PHP

[English](static-files.md) | [简体中文](static-files.zh-CN.md)

`static` 处理器提供一个目录；`fastcgi` 处理器经 PHP-FPM 运行 PHP，或任何 FastCGI 应用。两者都在 akio 的文件池上访问磁盘，从不在
服务连接的线程上，所以慢磁盘只拖慢需要它的请求。[配置参考](../reference/config.zh-CN.md#static)列出每个字段。

## 一个站点

```fit
listener web :8080

route {
	handler static {
		root public
		index index.html
	}
}
```

`static` 应答 `GET` 和 `HEAD`，带 `ETag` 和 `Last-Modified`，支持条件请求（304）和范围请求（206），并按文件扩展名给出内容类型。
不超过 `max_file_bytes` 的文件保存在内存中，总量不超过 `max_cache_bytes`，并且至多每 `check_interval` 与磁盘核对一次。路径中有
以点开头的段（`/.git/config`、`/.env`）答 404，`/.well-known/` 之下除外；`dotfiles allow` 提供这些文件。

## 单页应用

`try_files` 依次尝试各路径，提供第一个存在的：这里先是请求的文件，然后是目录的 index，然后是应用的 `index.html`，这样应用自己的
路由也会加载应用。

```fit
listener web :8080

route {
	handler static {
		root public
		try_files {path} {path}/ /index.html
	}
}
```

## 压缩

`precompressed` 向接受它的客户端提供事先压缩好的文件（`app.js.br`、`app.js.gz`）；`compress` 为保存在内存中的文件保留压缩副本，
在文件池上只压缩一次。`compress` 中间件在写出时压缩路由应答的其他内容。

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

## 在前缀之下，以及目录列表

`strip_prefix` 在自己的路径下提供一个目录；`browse` 列出没有 index 的目录。

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

一条经 `fastcgi` 运行脚本的路由，加上另一条处理其余文件的路由：`try_files` 把不对应脚本的路径交给前端控制器，这是大多数 PHP
框架需要的。PHP 的参数（`SCRIPT_FILENAME`、`PATH_INFO` 等）由 `root` 和 `split_path` 设置；`env` 可以再添加。到 PHP-FPM 的连接
是一个有界的池（`max_conns`、`max_idle_conns`），请求体和响应体双向流式转发。

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

把脚本路由写成 `(?i)\.php$`：在不区分大小写的文件系统上，`/x.PHP` 会打开 `x.php`。`static` 从不提供扩展名属于某个 `fastcgi`
处理器 `split_path` 的文件，所以在两者共用的根目录上，脚本的源码永远不会作为文本提供。
