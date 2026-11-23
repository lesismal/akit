// akit as a server under test in fit's benchmark harness (fitbench).
//
// akit's bench/stage.sh copies this file into fit's bench/cmd/fitbench, at the fit commit
// bench/FIT_REV pins, and akit's config into fit's bench/configs/akit; fit's own files are not
// changed. fitbench then runs akit like the other servers: from an image of its own (the
// -akit-image flag, which stage.sh's image akit-bench:<tag> is given to), pinned to the server's
// CPUs, its readiness, versions, memory and CPU read as for any server.
//
// Copyright the akit authors. Licensed under the Apache License, Version 2.0, as fit is.

package main

func init() {
	servers = append(servers, server{
		name:  "akit",
		image: "akit-bench:local",
		// What akit serves so far (docs/dev-plan.md): the fixed response, a static file (I12) and
		// the proxy (I15), over HTTP/1.1, h2c, HTTPS/1.1 and h2, and idle HTTP/1.1 and TLS
		// connections.
		scenarios: []string{"fixed", "static", "proxy"},
		idle:      []string{"http1", "tls"},
		mounts:    [][2]string{{"akit/fit.json", "/etc/akit/fit.json"}, {"www", "/srv/www"}, {"certs", "/certs"}},
		cmd:       []string{"/usr/local/bin/akit", "run", "-c", "/etc/akit/fit.json"},
		version:   []string{"/usr/local/bin/akit", "version"},
		ready:     "/fixed",
	})
}
