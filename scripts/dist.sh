#!/bin/sh
# dist.sh builds akit for the platforms it is released for and packs each binary with the LICENSE
# and READMEs, a .tar.gz per target in dist/, with their SHA-256 sums in dist/checksums.txt, as
# fit's scripts/dist.sh does. release.yml runs it for a tag; `make dist` runs it here. See
# deploy/README.md.
#
# Usage: scripts/dist.sh [version]
#
# The version (git describe when left out) is what `akit version` prints (AKIT_VERSION) and goes
# in each archive's name. TARGETS, space separated os/arch pairs, narrows the platforms; by
# default they are those this host can build:
#
#   linux/amd64, linux/arm64    static musl binaries, linked by rust-lld (any host)
#   darwin/amd64, darwin/arm64  macOS binaries (a macOS host, with Xcode's linker)
#
# The Linux binaries link nothing at run time: akit and akio are Rust alone, and musl's C library
# is linked in. KEEP=1 leaves dist/ as it is, adding to it, for a job that packs the archives of
# several hosts into one release.
set -eu

root=$(git rev-parse --show-toplevel)
cd "$root"

version=${1:-$(git describe --tags --always --dirty)}
case "$(uname -s)" in
Darwin) default="linux/amd64 linux/arm64 darwin/amd64 darwin/arm64" ;;
*) default="linux/amd64 linux/arm64" ;;
esac
targets=${TARGETS:-$default}

# triple names the Rust target of os/arch $1.
triple() {
	case "$1" in
	linux/amd64) echo x86_64-unknown-linux-musl ;;
	linux/arm64) echo aarch64-unknown-linux-musl ;;
	darwin/amd64) echo x86_64-apple-darwin ;;
	darwin/arm64) echo aarch64-apple-darwin ;;
	*) echo "dist: unknown target $1 (linux/amd64, linux/arm64, darwin/amd64, darwin/arm64)" >&2; return 1 ;;
	esac
}

if tar --version 2>/dev/null | grep -q 'GNU tar'; then
	owner="--owner=0 --group=0 --numeric-owner"
else
	# bsdtar (macOS): no extended attributes or AppleDouble files either.
	owner="--uid 0 --gid 0 --no-xattrs --no-mac-metadata"
	export COPYFILE_DISABLE=1
fi

[ "${KEEP:-}" = 1 ] || rm -rf dist
mkdir -p dist/.stage

for t in $targets; do
	rt=$(triple "$t")
	os=${t%/*}
	arch=${t#*/}
	name=akit_${version#v}_${os}_${arch}
	stage=dist/.stage/$name

	echo "dist: $name ($rt)"
	rustup target list --installed | grep -qx "$rt" || rustup target add "$rt"
	# rust-lld links the musl targets with the C runtime Rust ships for them, so no cross
	# toolchain is needed on any host.
	CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
		CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
		AKIT_VERSION="$version" \
		cargo build --release --locked -p akit --target "$rt"
	mkdir -p "$stage"
	cp "target/$rt/release/akit" "$stage/akit"
	cp LICENSE README.md README.zh-CN.md "$stage/"
	# Files owned by root, not by whoever built them, and nothing but the files.
	# shellcheck disable=SC2086
	tar -C dist/.stage $owner -czf "dist/$name.tar.gz" "$name"
done

rm -rf dist/.stage
cd dist
if command -v sha256sum >/dev/null; then
	sha256sum akit_*.tar.gz > checksums.txt
else
	shasum -a 256 akit_*.tar.gz > checksums.txt
fi
cat checksums.txt
