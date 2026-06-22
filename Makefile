test-rs:
	cargo test -- --test-threads=1

test-py:
	python3 -m pytest crates/dev/scripts/tests/ -v

test: test-rs test-py
