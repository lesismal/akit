#!/bin/sh
# check-dist.sh checks what scripts/dist.sh packed in dist/ (or the directory given): the
# checksums match, each archive holds akit, LICENSE and the READMEs under its own directory, each
# binary is built for the platform its name says, the Linux ones are static (no interpreter, no
# shared libraries), and the binary this host can run prints the archive's version. release.yml
# runs it before publishing; `make dist-check` runs it here.
#
# Usage: scripts/check-dist.sh [dir]
set -eu

dir=${1:-dist}
cd "$dir"
[ -f checksums.txt ] || { echo "check-dist: no checksums.txt in $dir" >&2; exit 1; }
if command -v sha256sum >/dev/null; then
	sha256sum -c checksums.txt
else
	shasum -a 256 -c checksums.txt
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

host_os=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$(uname -m)" in
x86_64 | amd64) host_arch=amd64 ;;
aarch64 | arm64) host_arch=arm64 ;;
*) host_arch=$(uname -m) ;;
esac

n=0
for a in akit_*.tar.gz; do
	[ -f "$a" ] || { echo "check-dist: no archives in $dir" >&2; exit 1; }
	grep -q " \*\{0,1\}$a\$" checksums.txt || { echo "check-dist: $a is not in checksums.txt" >&2; exit 1; }
	name=${a%.tar.gz}
	rest=${name#akit_}
	arch=${rest##*_}
	rest=${rest%_*}
	os=${rest##*_}
	version=${rest%_*}

	want="$name/
$name/LICENSE
$name/README.md
$name/README.zh-CN.md
$name/akit"
	got=$(tar -tzf "$a" | sort)
	[ "$got" = "$want" ] || { printf 'check-dist: %s holds\n%s\nnot\n%s\n' "$a" "$got" "$want" >&2; exit 1; }
	tar -C "$tmp" -xzf "$a"
	bin=$tmp/$name/akit
	[ -x "$bin" ] || { echo "check-dist: $a: akit is not executable" >&2; exit 1; }

	info=$(file -b "$bin")
	case "$os/$arch" in
	linux/amd64) pat='ELF 64-bit.*x86-64' ;;
	linux/arm64) pat='ELF 64-bit.*(ARM aarch64|aarch64)' ;;
	darwin/amd64) pat='Mach-O 64-bit.*x86_64' ;;
	darwin/arm64) pat='Mach-O 64-bit.*arm64' ;;
	*) echo "check-dist: $a: unknown platform $os/$arch" >&2; exit 1 ;;
	esac
	echo "$info" | grep -Eq "$pat" || { echo "check-dist: $a: akit is $info" >&2; exit 1; }
	if [ "$os" = linux ]; then
		echo "$info" | grep -Eq 'statically linked|static-pie linked' || { echo "check-dist: $a: not static: $info" >&2; exit 1; }
		if echo "$info" | grep -q 'interpreter'; then echo "check-dist: $a: has an interpreter: $info" >&2; exit 1; fi
	fi

	if [ "$os" = "$host_os" ] && [ "$arch" = "$host_arch" ]; then
		v=$("$bin" version | head -n 1)
		case "$v" in
		"akit $version" | "akit v$version") ;;
		*) echo "check-dist: $a: akit version says \"$v\", not akit $version" >&2; exit 1 ;;
		esac
		echo "check-dist: $a: ran: $v"
	fi
	echo "check-dist: $a: $os/$arch ok"
	n=$((n + 1))
done
echo "check-dist: $n archives ok"
