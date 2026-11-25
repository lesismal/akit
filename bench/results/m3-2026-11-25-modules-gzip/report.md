| Module | fit req/s | akit-epoll req/s | akit-uring req/s | fit p99 | akit-epoll p99 | akit-uring p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| none | 268,142 | 227,367 (85%) | 232,844 (87%) | 2.72 ms | 2.87 ms | 2.65 ms |
| compress | 265,511 | 220,495 (83%) | 226,298 (85%) | 3.12 ms | 2.97 ms | 2.66 ms |
| compress_gzip | 251,518 | 185,257 (74%) | 189,874 (75%) | 3.93 ms | 3.49 ms | 2.73 ms |

CPU per request (µs), and what the middleware adds to its server's route without one:

| Module | fit µs | akit-epoll µs | akit-uring µs | fit +µs | akit-epoll +µs | akit-uring +µs | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| none | 21.1 | 28.2 | 27.5 |  |  |  | 0 |
| compress | 22.3 | 28.9 | 28.3 | +1.2 | +0.7 | +0.8 | 0 |
| compress_gzip | 25.0 | 34.9 | 34.2 | +3.9 | +6.7 | +6.8 | 0 |
