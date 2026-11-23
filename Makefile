.PHONY: all build check test test-changed lint fmt fmt-check clippy deny docs-check

CARGO ?= cargo
BASE ?= origin/main

all: check build

build:
	$(CARGO) build --release -p akit

# What every change passes before a push (docs/rules.md rules 17, 25, 26).
check: fmt-check clippy test deny docs-check

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
