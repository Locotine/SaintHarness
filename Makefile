SHELL := /usr/bin/env bash
export PATH := $(HOME)/.cargo/bin:$(PATH)
export CARGO_BUILD_JOBS ?= 4

.PHONY: check fmt test clippy firewall cleanroom parity contracts hooks release release-full install uninstall desktop-check desktop-dev desktop-deb desktop-install all

check:
	cargo check --workspace --all-targets

fmt:
	cargo fmt --all -- --check

test:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run --workspace; \
	else \
		cargo test --workspace; \
	fi

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

firewall:
	./scripts/check-firewall.sh

cleanroom:
	./scripts/check-cleanroom.sh

parity:
	./scripts/check-prototype-parity.sh

contracts:
	python3 .github/scripts/test_ci_contracts.py

hooks:
	./scripts/install-hooks.sh

release:
	cargo build --release -p saint-harness-cli

release-full:
	cargo build --release -p saint-harness-cli --features full

install: release-full
	./scripts/install.sh --local target/release/saint-harness

uninstall:
	./scripts/install.sh --uninstall

desktop-check:
	cd apps/desktop/src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test

desktop-dev:
	cd apps/desktop && npm run dev

desktop-deb:
	cd apps/desktop && npm run build:deb

desktop-install: desktop-deb
	./scripts/install-desktop-linux.sh

all: contracts cleanroom firewall fmt clippy test parity
