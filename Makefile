.PHONY: all build check test test-changed lint fmt fmt-check clippy deny docs-check reference compat-check compat-gate compat-bless \
	dist dist-check image packaging-check

CARGO ?= cargo
BASE ?= origin/main

all: check build

build:
	$(CARGO) build --release -p akit

# The release's archives in dist/, with checksums.txt (scripts/dist.sh, deploy/README.md): static
# musl binaries for Linux (amd64, arm64) and, on macOS, macOS ones. VERSION names them (git
# describe when empty); TARGETS narrows them, as TARGETS="linux/amd64".
VERSION ?=
dist:
	scripts/dist.sh $(VERSION)
	scripts/check-dist.sh

# Checks what dist/ holds: checksums, contents, platforms, static Linux binaries.
dist-check:
	scripts/check-dist.sh

# The packaging's shell scripts parse (crates/akit/tests/packaging.rs checks the files' contents in
# `make test`; CI's packaging job builds and runs them).
PACKAGING_SCRIPTS = scripts/dist.sh scripts/check-dist.sh deploy/systemd/test.sh
packaging-check:
	@for f in $(PACKAGING_SCRIPTS); do sh -n "$$f" || exit 1; done
	@if command -v shellcheck >/dev/null; then shellcheck $(PACKAGING_SCRIPTS); fi
	@echo "packaging: scripts parse"

# The container image, for this machine's platform (Dockerfile, deploy/README.md).
IMAGE ?= akit:local
image:
	docker build $(if $(VERSION),--build-arg VERSION=$(VERSION)) -t $(IMAGE) .

# What every change passes before a push (docs/rules.md rules 17, 25, 26).
check: fmt-check clippy test deny docs-check compat-check packaging-check

test:
	$(CARGO) test --workspace

# The crates changed since BASE and those that depend on them: the check while working.
test-changed:
	scripts/test-changed.sh $(BASE)

lint: fmt-check clippy

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

deny:
	$(CARGO) deny check

docs-check:
	scripts/check-docs.sh
	python3 -I tools/config-reference.py --check

# Rewrites the config reference's tables (docs/reference/config.md and its twin) from fit's pinned
# schema, tests/compat/supported.txt and tools/config-reference-notes.json.
reference:
	python3 -I tools/config-reference.py

# The compatibility gate's field inventory and docs/compatibility.md are up to date with the pinned
# fit schema and tests/compat/supported.txt (docs/compatibility.md).
compat-check:
	python3 -I tools/compat-inventory.py --check

# The gate (I07b) runs in `make test` and CI with every other test; this runs it alone and prints
# its report. The examples' secrets are set to stand-ins as fit-examples' own test sets them, for
# when akit reads the examples (I23).
COMPAT_SECRETS = ES_PASSWORD KAFKA_PASSWORD MYSQL_PASSWORD PG_PASSWORD RABBITMQ_PASSWORD \
	NATS_PASSWORD PULSAR_TOKEN MQTT_PASSWORD INFLUX_TOKEN TD_PASSWORD MSSQL_PASSWORD \
	DORIS_PASSWORD OPENAI_API_KEY QDRANT_API_KEY MILVUS_TOKEN
compat-gate:
	env $(foreach v,$(COMPAT_SECRETS),$(v)=example-password) \
		$(CARGO) test -p akit-compat --test gate gate_through_akit_config -- --nocapture

# Rewrites tests/compat/gate-report.txt from a passing run of the gate: for the commit that
# changes an outcome (a field supported, a reader added). Refuses while any fixture fails.
compat-bless:
	env AKIT_COMPAT_BLESS=1 $(foreach v,$(COMPAT_SECRETS),$(v)=example-password) \
		$(CARGO) test -p akit-compat --test gate gate_through_akit_config -- --nocapture
