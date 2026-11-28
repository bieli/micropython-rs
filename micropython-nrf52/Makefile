# micropython-rs firmware for nRF52840 boards with the Adafruit UF2
# bootloader (nice!nano v2, SuperMini nRF52840). Builds with stable Rust;
# flashing is a copy of a UF2 file to the bootloader's USB drive (no probe).
#
#   make flash        # build, convert to UF2, copy it to the board
#   make build | uf2 | clippy | size
#
# The bootloader's drive (NICENANO) must be there: on a board without a reset
# button, short RST to GND twice quickly.
#
# Needs: rustup target add thumbv7em-none-eabihf

TARGET := thumbv7em-none-eabihf
ELF    := target/$(TARGET)/release/micropython-nrf52
UF2    := target/micropython-nrf52.uf2
# Where the desktop mounts the bootloader's drive
DRIVE  ?= /media/$(USER)/NICENANO

GIT_COMMIT := $(shell git -C .. describe --always --dirty --abbrev=7 --exclude='*' 2>/dev/null)

.PHONY: all build uf2 flash clippy size clean

all: flash

build:
	MPY_GIT_COMMIT="$(GIT_COMMIT)" cargo build --release

uf2: build
	python3 tools/uf2.py $(ELF) $(UF2)

flash: uf2
	@test -f "$(DRIVE)/INFO_UF2.TXT" || { echo "No bootloader drive at $(DRIVE): short RST to GND twice quickly (or DRIVE=...)"; exit 1; }
	cp $(UF2) "$(DRIVE)/"
	sync
	@echo "Copied; the bootloader writes it and starts the firmware."

clippy:
	MPY_GIT_COMMIT="$(GIT_COMMIT)" cargo clippy --release -- -D warnings

size: build
	python3 tools/size.py $(ELF)

clean:
	cargo clean
