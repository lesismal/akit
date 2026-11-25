#!/usr/bin/env python3
"""Summarizes a bench/modules.sh run (results.tsv) as Markdown tables: for each module and
server, the median over the rounds of requests a second, wrk's p99 and the server's CPU per
request, and the CPU each middleware adds to its server's route without one ("none")."""

import statistics
import sys
from collections import defaultdict

ORDER = ["fit", "akit-epoll", "akit-uring"]


def main(path):
    rows = defaultdict(list)
    modules, servers = [], set()
    with open(path) as f:
        head = f.readline().rstrip("\n").split("\t")
        for line in f:
            r = dict(zip(head, line.rstrip("\n").split("\t")))
            key = (r["module"], r["server"])
            rps, secs, cpu = float(r["rps"]), float(r["seconds"]), float(r["cpu_s"])
            us = cpu / secs / rps * 1e6 if rps > 0 and secs > 0 else float("nan")
            rows[key].append((rps, float(r["p99_ms"]), us, int(r["bad"])))
            if r["module"] not in modules:
                modules.append(r["module"])
            servers.add(r["server"])
    servers = [s for s in ORDER if s in servers] + sorted(servers - set(ORDER))

    def med(m, s, i):
        v = [x[i] for x in rows.get((m, s), [])]
        return statistics.median(v) if v else float("nan")

    base = servers[0]
    print("| Module | " + " | ".join(f"{s} req/s" for s in servers) + " | "
          + " | ".join(f"{s} p99" for s in servers) + " |")
    print("| --- |" + " ---: |" * (2 * len(servers)))
    for m in modules:
        cells = []
        for s in servers:
            v = med(m, s, 0)
            share = "" if s == base else f" ({v / med(m, base, 0):.0%})"
            cells.append(f"{v:,.0f}{share}")
        cells += [f"{med(m, s, 1):.2f} ms" for s in servers]
        print(f"| {m} | " + " | ".join(cells) + " |")
    print()
    print("CPU per request (µs), and what the middleware adds to its server's route without one:")
    print()
    print("| Module | " + " | ".join(f"{s} µs" for s in servers) + " | "
          + " | ".join(f"{s} +µs" for s in servers) + " | Errors |")
    print("| --- |" + " ---: |" * (2 * len(servers) + 1))
    for m in modules:
        us = [med(m, s, 2) for s in servers]
        add = ["" if m == "none" else f"{u - med('none', s, 2):+.1f}" for u, s in zip(us, servers)]
        bad = sum(x[3] for s in servers for x in rows.get((m, s), []))
        print(f"| {m} | " + " | ".join(f"{u:.1f}" for u in us) + " | " + " | ".join(add)
              + f" | {bad} |")


if __name__ == "__main__":
    main(sys.argv[1])
