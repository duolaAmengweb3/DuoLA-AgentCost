.PHONY: fmt check test lint e2e release verify

fmt:
	cargo fmt --all

check:
	cargo check --all-targets --all-features

test:
	cargo test --all-targets --all-features

lint:
	cargo clippy --all-targets --all-features -- -D warnings

e2e:
	./tests/e2e.sh

release:
	cargo build --release

verify: fmt check lint test e2e release
