# Shared by bench/run.sh and bench/ab.sh: staging fit's harness and akit builds on the
# benchmark host. Sourced, not run.
#
# The host (BENCH_SSH) has Go, Docker and cargo, so everything is built there: fit's tree at
# bench/FIT_REV, from the fit repository beside akit's (FIT_REPO), with akit's overlay
# (bench/fitbench/akit.go) and configs (bench/configs/akit, and the wasm scenario's plugin) added;
# fitbench, idleconn and fit built with Go; akit built with cargo, at the ref asked for, into an
# image of its own, akit-bench:<tag>. Builds take /root/bench.lock shared; measured runs take it
# alone, through the host's quiet.sh (docs/rules.md, rule 23).

BENCH_SSH=${BENCH_SSH:-"ssh root@gpu51"}
BENCH_DIR=${BENCH_DIR:-/root/akit-bench}
QUIET=${QUIET:-/root/akio-bench/quiet.sh}
root=$(git rev-parse --show-toplevel)
here="$root/bench"
FIT_REPO=${FIT_REPO:-"$root/../fit"}
fit_rev=$(head -n 1 "$here/FIT_REV" | tr -cd '0-9a-f')
host_path='export PATH=$HOME/.cargo/bin:/usr/local/go/bin:$PATH'

die() {
	echo "$(basename "$0"): $*" >&2
	exit 1
}

# stage_fit puts fit's tree at FIT_REV on the host, once, with akit's overlay and config copied
# in each time, and builds fitbench, benchreport, idleconn and fit into its bin/.
stage_fit() {
	git -C "$FIT_REPO" cat-file -e "$fit_rev^{commit}" 2>/dev/null ||
		die "fit $fit_rev is not in $FIT_REPO (set FIT_REPO, or fetch it there)"
	fit_dir="$BENCH_DIR/fit-$(echo "$fit_rev" | cut -c1-12)"
	if ! $BENCH_SSH "test -e $fit_dir/.staged"; then
		echo "bench: uploading fit $(echo "$fit_rev" | cut -c1-8)"
		git -C "$FIT_REPO" archive --format=tar "$fit_rev" |
			$BENCH_SSH "mkdir -p $fit_dir && tar xf - -C $fit_dir && touch $fit_dir/.staged"
	fi
	COPYFILE_DISABLE=1 tar --no-xattrs -cf - -C "$here" fitbench/akit.go configs/akit |
		$BENCH_SSH "cd $fit_dir/bench && tar xf - && mv fitbench/akit.go cmd/fitbench/akit.go && rmdir fitbench"
	# The wasm scenario's plugin (fitbench/akit.go), fit's own test plugin as akit's tests run it:
	# built for akit's ABI for akit, and fit's own build for fit (decision 037).
	gzip -dc "$root/crates/akit-plugin/tests/guests/headers-akit.wasm.gz" |
		$BENCH_SSH "cat > $fit_dir/bench/configs/akit/headers.wasm"
	gzip -dc "$root/crates/akit-plugin/tests/guests/headers-fit.wasm.gz" |
		$BENCH_SSH "cat > $fit_dir/bench/configs/akit/fit-headers.wasm"
	echo "bench: building fitbench, idleconn and fit on the host"
	$BENCH_SSH "$host_path; cd $fit_dir && mkdir -p bin && flock -s /root/bench.lock sh -c '
		go build -o bin/fitbench ./bench/cmd/fitbench &&
		go build -o bin/benchreport ./bench/cmd/benchreport &&
		go build -o bin/idleconn ./bench/cmd/idleconn &&
		CGO_ENABLED=0 go build -o bin/fit ./cmd/fit'" ||
		die "building fit's harness on the host failed"
}

# stage_akit REF TAG [ENV] builds akit at REF (a git ref, or "work" for the working tree) on the
# host and makes the image akit-bench:TAG from it, with ENV (one VAR=VALUE, no spaces or quotes)
# in the image's environment when given; prints what REF was.
stage_akit() {
	ref=$1
	tag=$2
	env=${3:+ENV $3}
	src="$BENCH_DIR/src-$tag"
	$BENCH_SSH "rm -rf $src && mkdir -p $src"
	if [ "$ref" = work ]; then
		desc="work (uncommitted changes of $(git -C "$root" rev-parse --short HEAD))"
		(cd "$root" && COPYFILE_DISABLE=1 tar --no-xattrs -cf - --exclude ./target --exclude ./.git \
			--exclude ./.claude --exclude ./fuzz/target --exclude ./bench/results .) |
			$BENCH_SSH "tar xf - -C $src"
	else
		desc="$ref ($(git -C "$root" rev-parse --short "$ref"))"
		# Extracted with the current time (m): git archive stamps files with the commit's time, and
		# every build shares one target directory, so files older than the last build's outputs
		# would let cargo reuse what another ref compiled.
		git -C "$root" archive --format=tar "$ref" | $BENCH_SSH "tar xmf - -C $src"
	fi
	echo "bench: building akit $desc on the host" >&2
	# One target directory for every build, so a build reuses what the last compiled.
	$BENCH_SSH "$host_path; cd $src && flock -s /root/bench.lock \
		cargo build -q --release -p akit --target-dir $BENCH_DIR/target &&
		mkdir -p image && cp $BENCH_DIR/target/release/akit image/akit &&
		printf 'FROM ubuntu:24.04\nCOPY akit /usr/local/bin/akit\n$env\n' > image/Dockerfile &&
		docker build -q -t akit-bench:$tag image >/dev/null" >&2 ||
		die "building akit $desc on the host failed"
	echo "$desc"
}

# image_akit FROM TAG [ENV] makes the image akit-bench:TAG from the binary stage_akit built for
# the tag FROM, with ENV in its environment: the same binary, run another way.
image_akit() {
	env=${3:+ENV $3}
	$BENCH_SSH "cd $BENCH_DIR/src-$1 && mkdir -p image-$2 && cp image/akit image-$2/akit &&
		printf 'FROM ubuntu:24.04\nCOPY akit /usr/local/bin/akit\n$env\n' > image-$2/Dockerfile &&
		docker build -q -t akit-bench:$2 image-$2 >/dev/null" >&2 ||
		die "making the image akit-bench:$2 on the host failed"
}
