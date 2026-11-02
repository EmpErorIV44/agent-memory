.DEFAULT_GOAL := help

CARGO ?= cargo
PYTHON ?= python3
JOBS ?= 1
ARGS ?=

.PHONY: help build release run test test-python fmt fmt-check lint check clean

help:
	@printf '%s\n' \
	  'make build        Build the workspace in debug mode' \
	  'make release      Build the release binary' \
	  'make run ARGS=... Run the CLI with optional arguments' \
	  'make test         Run all Rust tests sequentially' \
	  'make test-python  Test release packaging' \
	  'make fmt          Format Rust sources' \
	  'make fmt-check    Check Rust formatting' \
	  'make lint         Run Clippy with warnings denied' \
	  'make check        Run formatting, lint, packaging and Rust tests' \
	  'make clean        Remove Cargo build artifacts' \
	  '' \
	  'Overrides: CARGO=cargo PYTHON=python3 JOBS=1 ARGS="--help"'

build:
	$(CARGO) build --workspace --locked -j "$(JOBS)"

release:
	$(CARGO) build -p agent-memory --release --locked -j "$(JOBS)"

run:
	$(CARGO) run -p agent-memory --locked -j "$(JOBS)" -- $(ARGS)

test:
	@tmpdir="$$( $(PYTHON) -c 'import os, tempfile; print(os.path.realpath(tempfile.gettempdir()))')" && \
	  TMPDIR="$$tmpdir" $(CARGO) test --workspace --locked -j "$(JOBS)" -- --test-threads=1

test-python:
	$(PYTHON) -m unittest discover -s tests -p 'test_package_release.py'

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --workspace --all-targets --locked -j "$(JOBS)" -- -D warnings

# Keep checks ordered even when the caller uses make -j.
check:
	$(MAKE) fmt-check
	$(MAKE) lint
	$(MAKE) test-python
	$(MAKE) test

clean:
	$(CARGO) clean
