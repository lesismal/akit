.PHONY: all build check test test-changed lint fmt fmt-check clippy deny docs-check compat-check compat-gate

CARGO ?= cargo
BASE ?= origin/main

all: check build

build:
	$(CARGO) build --release -p akit

# What every change passes before a push (docs/rules.md rules 17, 25, 26).
check: fmt-check clippy test deny docs-check compat-check

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

# The compatibility gate's field inventory and docs/compatibility.md are up to date with the pinned
# fit schema and tests/compat/supported.txt (docs/compatibility.md).
compat-check:
	python3 -I tools/compat-inventory.py --check

# The gate through akit_config::from_json_file (I07b), with the examples' secrets set to stand-ins as
# fit-examples' own test sets them.
COMPAT_SECRETS = ES_PASSWORD KAFKA_PASSWORD MYSQL_PASSWORD PG_PASSWORD RABBITMQ_PASSWORD \
	NATS_PASSWORD PULSAR_TOKEN MQTT_PASSWORD INFLUX_TOKEN TD_PASSWORD MSSQL_PASSWORD \
	DORIS_PASSWORD OPENAI_API_KEY QDRANT_API_KEY MILVUS_TOKEN
compat-gate:
	env $(foreach v,$(COMPAT_SECRETS),$(v)=example-password) \
		$(CARGO) test -p akit-compat --features compat-gate -- --nocapture
