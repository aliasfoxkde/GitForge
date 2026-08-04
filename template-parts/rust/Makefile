.PHONY: build test test-unit test-integration clippy fmt check clean run

build:
	cargo build --release

test: test-unit test-integration

test-unit:
	cargo test --test unit

test-integration:
	cargo test --test integration

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

check: fmt clippy build

clean:
	cargo clean

run:
	cargo run --release
