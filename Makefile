PYTHON ?= python3.15

test-rs:
	cargo test -- --test-threads=1

test-py:
	$(PYTHON) -m pytest crates/dev/scripts/tests/ -v

test: test-rs test-py
