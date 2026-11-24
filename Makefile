# Target configuration
MIN_COVERAGE := 30
EMBEDDED_ARM := thumbv7em-none-eabihf
EMBEDDED_ESP32 := riscv32imc-unknown-none-elf

FILE ?= hello.py

.PHONY: all check test coverage run build-embedded levels clean cpython-expected

# Default target: run host tests, enforce coverage, and verify bare-metal compilation
all: test coverage build-embedded

# Fast syntax and type check for both host and embedded targets
check:
	cargo check
	cargo check --lib --target $(EMBEDDED_ARM)
	cargo check --lib --target $(EMBEDDED_ESP32)

# Run unit tests on the host
test:
	cargo test

# Enforce minimum line coverage threshold (exits non-zero if below MIN_COVERAGE)
coverage:
	cargo llvm-cov --lib --fail-under-lines $(MIN_COVERAGE)

# Run the host CLI runner (src/main.rs) with a Python script
run:
	cargo run -- $(FILE)

# Verify that src/lib.rs compiles cleanly in strict #![no_std] mode
build-embedded:
	cargo build --lib --target $(EMBEDDED_ARM)
	cargo build --lib --target $(EMBEDDED_ESP32)

# The smaller feature levels (the default is full): lints, tests and the
# bare-metal builds of each
levels:
	for level in minimal medium; do \
		cargo clippy --all-targets --no-default-features --features $$level -- -D warnings && \
		cargo test --no-default-features --features $$level && \
		cargo build --lib --target $(EMBEDDED_ARM) --no-default-features --features $$level || exit 1; \
	done

# Regenerate the expected output of the CPython conformance scripts (needs python3)
cpython-expected:
	cd tests/cpython && for f in *.py; do python3 -B -W ignore "$$f" > "$${f%.py}.out"; done

# Remove build artifacts
clean:
	cargo clean
