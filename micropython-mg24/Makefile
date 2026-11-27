# micropython-rs firmware for the Seeed XIAO MG24 (EFR32MG24, Cortex-M33).
# Builds with stable Rust (no Docker), flashes with probe-rs through the
# board's own CMSIS-DAP debugger (USB-C; no BOOT button needed).
#
#   make flash               # build, flash, reset
#   make build | clippy | size
#   make flash LEVEL=medium  # the interpreter's level: full (default), medium, minimal
#   make flash FLOAT=64      # 64-bit floats (in software; the FPU is single precision)
#
# Needs: rustup target add thumbv8m.main-none-eabihf; cargo install probe-rs-tools

CHIP_NAME := EFR32MG24B220F1536IM48
TARGET    := thumbv8m.main-none-eabihf
ELF       := target/$(TARGET)/release/micropython-mg24
PROBE_RS  := $(HOME)/.cargo/bin/probe-rs

LEVEL     ?= full
FLOAT     ?= 32
comma     := ,
FEATURES_FLAG := --no-default-features --features $(LEVEL)$(if $(filter 32,$(FLOAT)),$(comma)float32)

# The git commit the firmware is built from (`-dirty` with uncommitted
# changes to tracked files): the banner, lscpu() and tools/mpy.py show it
GIT_COMMIT := $(shell git -C .. describe --always --dirty --abbrev=7 --exclude='*' 2>/dev/null)

.PHONY: all build flash reset clippy size clean

all: flash

build:
	MPY_GIT_COMMIT="$(GIT_COMMIT)" cargo build --release $(FEATURES_FLAG)

flash: build
	$(PROBE_RS) download --chip $(CHIP_NAME) $(ELF)
	$(PROBE_RS) reset --chip $(CHIP_NAME)

reset:
	$(PROBE_RS) reset --chip $(CHIP_NAME)

clippy:
	MPY_GIT_COMMIT="$(GIT_COMMIT)" cargo clippy --release $(FEATURES_FLAG) -- -D warnings

# Flash and RAM use: the sections of the ELF (no extra tools needed)
size: build
	python3 tools/size.py $(ELF)

clean:
	cargo clean
