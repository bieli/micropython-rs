# Target configuration
MIN_COVERAGE := 30
EMBEDDED_ARM := thumbv7em-none-eabihf
EMBEDDED_ESP32 := riscv32imc-unknown-none-elf	

FILE ?= hello.py

.PHONY: all check test coverage run build-embedded clean

# Default target: run host tests, enforce coverage, and verify bare-metal compilation
all: test coverage build-embedded

# Fast syntax and type check for both host and embedded targets
check:
	cargo check
	cargo check --lib --target $(EMBEDDED_TARGET)

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

# Remove build artifacts
clean:
	cargo clean
