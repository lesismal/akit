# syntax=docker/dockerfile:1
# akit's container image: akit alone, a static musl binary on distroless's static image, run as
# its nonroot user (65532), as fit's image is. See deploy/README.md.
#
#	docker build -t akit .
#	docker buildx build --platform linux/amd64,linux/arm64 --build-arg VERSION=v0.1.0 -t akit .
#
# The binary is cross-compiled on the builder's platform for each target and linked by rust-lld,
# as scripts/dist.sh does, so a multi-platform build needs no emulation.
ARG RUST_VERSION=1.98

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim AS build
WORKDIR /src
COPY . .
# The image's toolchain builds akit: rust-toolchain.toml would fetch the latest stable, with
# clippy and rustfmt, on every build.
RUN rm -f rust-toolchain.toml
ARG TARGETARCH VERSION
RUN --mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/usr/local/cargo/git \
	--mount=type=cache,target=/src/target,id=akit-target-$TARGETARCH \
	set -eu; \
	case "$TARGETARCH" in \
	amd64) t=x86_64-unknown-linux-musl ;; \
	arm64) t=aarch64-unknown-linux-musl ;; \
	*) echo "akit's image is built for amd64 and arm64, not $TARGETARCH" >&2; exit 1 ;; \
	esac; \
	rustup target add "$t"; \
	CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
	CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
	AKIT_VERSION="${VERSION:-}" \
	cargo build --release --locked -p akit --target "$t"; \
	mkdir -p /out; \
	cp "target/$t/release/akit" /out/akit

FROM gcr.io/distroless/static-debian12:nonroot
LABEL org.opencontainers.image.title="akit" \
	org.opencontainers.image.description="A reverse proxy, load balancer and API gateway on akio: fit rebuilt in Rust" \
	org.opencontainers.image.source="https://github.com/lesismal/akit" \
	org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build /out/akit /usr/local/bin/akit
COPY LICENSE /usr/share/doc/akit/LICENSE
COPY deploy/docker/akit.json /etc/akit/akit.json
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/akit"]
CMD ["run", "-c", "/etc/akit/akit.json"]
