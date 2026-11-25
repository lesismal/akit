// akit as a server under test in fit's benchmark harness (fitbench).
//
// akit's bench/stage.sh copies this file into fit's bench/cmd/fitbench, at the fit commit
// bench/FIT_REV pins, and akit's config into fit's bench/configs/akit; fit's own files are not
// changed. fitbench then runs akit like the other servers: from an image of its own (the
// -akit-image flag, which stage.sh's image akit-bench:<tag> is given to), pinned to the server's
// CPUs, its readiness, versions, memory and CPU read as for any server.
//
// It also adds the wasm scenario (I76): the fixed response behind the wasm middleware running
// fit's own header-editing test plugin (plugin/wasm/testdata/headers, akit's
// crates/akit-plugin/tests/guests/headers-go.wasm.gz, which lib.sh unpacks into the configs as
// akit/headers.wasm). Two servers serve it, akit-wasm and fit-wasm: akit and fit with their
// harness configs and one more config file each, which adds the route /wasm, so that the same
// .wasm runs in both, and the fixed scenario measured in the same container is the cost's
// baseline. They are servers of their own so that a run that does not name them loads no plugin
// (each instance of a Go guest holds its own memory, which would count against the server's).
//
// Copyright the akit authors. Licensed under the Apache License, Version 2.0, as fit is.

package main

func init() {
	akit := server{
		name:  "akit",
		image: "akit-bench:local",
		// What akit serves so far (docs/dev-plan.md): the fixed response, a static file (I12),
		// the proxy (I15) and the cache scenarios (I67-I69, in the harness since I76), over
		// HTTP/1.1, h2c, HTTPS/1.1 and h2, and idle HTTP/1.1, TLS and WebSocket (I49, in the
		// harness since I25) connections.
		scenarios: []string{"fixed", "static", "proxy", "cache-hot", "cache-zipf"},
		idle:      []string{"http1", "tls", "ws"},
		mounts:    [][2]string{{"akit/fit.json", "/etc/akit/fit.json"}, {"www", "/srv/www"}, {"certs", "/certs"}},
		cmd:       []string{"/usr/local/bin/akit", "run", "-c", "/etc/akit/fit.json"},
		version:   []string{"/usr/local/bin/akit", "version"},
		ready:     "/fixed",
	}
	servers = append(servers, akit)

	scenarios = append(scenarios, scenario{name: "wasm", path: "/wasm", body: "hello, world"})
	akitWasm := akit
	akitWasm.name = "akit-wasm"
	akitWasm.scenarios = []string{"fixed", "wasm"}
	akitWasm.idle = nil
	akitWasm.mounts = append(append([][2]string{}, akit.mounts...),
		[2]string{"akit/wasm.json", "/etc/akit/wasm.json"}, [2]string{"akit/headers.wasm", "/etc/akit/headers.wasm"})
	akitWasm.cmd = append(append([]string{}, akit.cmd...), "-c", "/etc/akit/wasm.json")
	servers = append(servers, akitWasm)
	for _, s := range servers {
		if s.name == "fit" {
			fitWasm := s
			fitWasm.name = "fit-wasm"
			fitWasm.scenarios = []string{"fixed", "wasm"}
			fitWasm.idle = nil
			fitWasm.mounts = append(append([][2]string{}, s.mounts...),
				[2]string{"akit/fit-wasm.conf", "/etc/fit/wasm.conf"}, [2]string{"akit/headers.wasm", "/etc/fit/headers.wasm"})
			fitWasm.cmd = append(append([]string{}, s.cmd...), "-c", "/etc/fit/wasm.conf")
			servers = append(servers, fitWasm)
			break
		}
	}
}
