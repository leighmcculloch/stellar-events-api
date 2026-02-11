.PHONY: build test run clean deploy

build:
	cargo build --release

test:
	cargo test

run:
	cargo run --release

clean:
	cargo clean
	rm -rf data/

deploy:
	fly deploy --ha=false --local-only --config fly.main.toml
	fly scale count 1
