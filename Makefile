# Target configuration
EMBEDDED_TARGET := thumbv7em-none-eabihf

FILE ?= hello.py

.PHONY: all check test run build-embedded clean

# Default target: run host tests and verify bare-metal compilation
all: test build-embedded

# Fast syntax and type check for both host and embedded targets
check:
	cargo check
	cargo check --lib --target $(EMBEDDED_TARGET)

# Run unit tests on the host
test:
	cargo test

# Run the host CLI runner (src/main.rs) with a Python script
run:
	cargo run -- $(FILE)

# Verify that src/lib.rs compiles cleanly in strict #![no_std] mode
build-embedded:
	cargo build --lib --target $(EMBEDDED_TARGET)

# Remove build artifacts
clean:
	cargo clean
