# akit with Docker Compose

[English](README.md) | [简体中文](README.zh-CN.md)

fit's [docker-compose example](https://github.com/lesismal/fit-examples/tree/main/docker-compose)
for akit: akit, built from this checkout's [Dockerfile](../../Dockerfile), in front of two replicas
of [whoami](https://github.com/traefik/whoami), a small app that answers with its container's name.
akit finds them with its Docker provider (I58): the app's labels in [compose.yaml](compose.yaml)
are all of its routing, and akit follows the containers as they start and stop.
[akit.conf](akit.conf) has akit's listener, the provider (reading labels under `akit.`), and a 404
for every other host.

```bash
cd deploy/compose
docker compose up -d --build
curl -H 'Host: whoami.localhost' http://127.0.0.1:8080/
```

whoami answers with `Hostname: <container>`; ask again and the other replica answers, once both
have passed their first health check. The labels say:

| Label | Means |
| --- | --- |
| `akit.enable=true` | Take this container. |
| `akit.port=80` | Send to its port 80. |
| `akit.routes.web.host=whoami.localhost` | A route for this host, proxying to the upstream of the app's containers, `akit-example-whoami` (the Compose project and service). |
| `akit.upstream={"health_check": ...}` | More of the upstream's settings, as in the config: here, an active health check. |

The prefix is the provider's `label_prefix`, `akit` when left out (fit's is `fit`: akit renamed
it, [decision 037](../../docs/decisions/037-akit-names.md)); a fit example's `fit.` labels work
with `label_prefix fit`.

Scale the app and akit follows:

```bash
docker compose up -d --no-build --scale whoami=3 whoami
docker compose stop whoami      # whoami.localhost now gets 404
docker compose start whoami
docker compose down
```

akit runs as root here because the Docker socket is root's; see [deploy](../README.md) for a socket
proxy instead. [test.sh](test.sh) runs these steps and checks each answer: both replicas answer, a
third answers once scaled up, other hosts get the config's 404, the app's host gets 404 once its
containers stop and 200 again once they start. CI's `packaging` job runs it. `AKIT_IMAGE=<image>`
runs an image built elsewhere instead of building one, and `AKIT_PORT` moves the published port.
