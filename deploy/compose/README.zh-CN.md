# 用 Docker Compose 运行 akit

[English](README.md) | [简体中文](README.zh-CN.md)

fit 的 [docker-compose 示例](https://github.com/lesismal/fit-examples/tree/main/docker-compose)
的 akit 版：由本检出的 [Dockerfile](../../Dockerfile) 构建的 akit，位于
[whoami](https://github.com/traefik/whoami) 的两个副本之前；whoami 是一个用容器名作答的小应用。
akit 用它的 Docker provider（I58）找到它们：[compose.yaml](compose.yaml) 中应用的标签就是它的全部
路由，akit 在容器启动和停止时跟随变化。[akit.conf](akit.conf) 中是 akit 的监听器、provider（读取
`akit.` 下的标签），以及对其他任何主机返回的 404。

```bash
cd deploy/compose
docker compose up -d --build
curl -H 'Host: whoami.localhost' http://127.0.0.1:8080/
```

whoami 以 `Hostname: <container>` 作答；再请求一次，另一个副本作答（两者都通过首次健康检查之后）。
标签的含义：

| 标签 | 含义 |
| --- | --- |
| `akit.enable=true` | 接管这个容器。 |
| `akit.port=80` | 发往它的 80 端口。 |
| `akit.routes.web.host=whoami.localhost` | 这个主机的一条路由，代理到应用容器的上游 `akit-example-whoami`（Compose 项目名加服务名）。 |
| `akit.upstream={"health_check": ...}` | 上游的更多设置，与配置中相同：这里是主动健康检查。 |

前缀是 provider 的 `label_prefix`；fit 的默认值 `fit` 也是 akit 的默认值，所以 provider 不写它时，
fit 示例的 `fit.` 标签可原样使用。

扩缩应用，akit 跟随：

```bash
docker compose up -d --no-build --scale whoami=3 whoami
docker compose stop whoami      # whoami.localhost 现在得到 404
docker compose start whoami
docker compose down
```

这里 akit 以 root 运行，因为 Docker socket 属于 root；改用 socket 代理见 [deploy](../README.zh-CN.md)。
[test.sh](test.sh) 执行这些步骤并检查每个回答：两个副本都作答，扩容后第三个作答，其他主机得到配置
中的 404，应用的主机在其容器停止后得到 404、再启动后重新得到 200。CI 的 `packaging` 作业运行它。
`AKIT_IMAGE=<image>` 运行在别处构建的镜像而不构建，`AKIT_PORT` 改变发布的端口。
