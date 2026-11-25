### fixed, HTTP/1.1

| Server |     TPS [↓1] | CPU EER [↓2] | MEM EER [↓3] |     p99 |  CPU | Peak memory | KiB/in-flight | Errors |
| ------ | -----------: | -----------: | -----------: | ------: | ---: | ----------: | ------------: | -----: |
| akit   | 815,380 100% | 130,655 100% | 183,264 100% | 0.37 ms | 6.24 |       4 MiB |           1.5 |      0 |
| nginx  | 806,155  98% | 126,006  96% |   4,086   2% | 0.35 ms | 6.40 |     197 MiB |           1.2 |      0 |
| fit    | 764,048  93% | 119,829  91% |  37,921  20% | 2.72 ms | 6.38 |      20 MiB |          52.0 |      0 |
| caddy  | 326,406  40% |  51,506  39% |  10,564   5% | 5.29 ms | 6.34 |      31 MiB |          80.1 |      0 |

Not run: traefik, which cannot serve it.

### Idle connections, bytes each

| Server  |  http1 [↑1] |    tls [↑2] |
| ------- | ----------: | ----------: |
| nginx   |    530 100% | 29,582  28% |
| akit    |  1,132  46% | 12,889  66% |
| fit     |  1,489  35% |  8,523 100% |
| caddy   | 20,454   2% | 38,002  22% |
| traefik | 21,864   2% | 39,929  21% |

With 100,000 connections of each kind; lower is better.

