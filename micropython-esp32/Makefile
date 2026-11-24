# Docker and target configuration
DOCKER_IMAGE := espressif/idf-rust:esp32_latest
TARGET       := xtensa-esp32-none-elf
BIN_NAME     := micropython-esp32
ELF_PATH     := target/$(TARGET)/release/$(BIN_NAME)

# Ensure Cargo-installed host binaries (~/.cargo/bin) are always found by Make subshells
ESPFLASH     := $(HOME)/.cargo/bin/espflash

# Optional port override (e.g., make flash PORT=/dev/ttyACM0); leave empty to use espflash's saved port
PORT         ?=
PORT_FLAG    := $(if $(PORT),--port $(PORT),)

# Optional Cargo features (e.g., make flash FEATURES=tft for the LilyGO TTGO T-Display);
# leave empty to build the generic ESP32 firmware (serial console + GPIO2 LED)
FEATURES     ?=
FEATURES_FLAG := $(if $(FEATURES),--features $(FEATURES),)

# Optional Python script to embed (e.g., make flash SCRIPT=scripts/blink.py), relative to this
# directory; leave empty for the board's demo (scripts/demo.py or scripts/ttgo_demo.py)
SCRIPT       ?=

# CPU clock in MHz: 80 (default, least current), 160 or 240 (fastest)
CPU_MHZ      ?=

# Partition table with the `vfs` file system partition (main.py and other files)
PARTITIONS   := partitions.csv

# Files in the `vfs` partition survive `make flash`; ERASE_FILES=1 deletes them all
# (then the embedded SCRIPT runs at boot, as no main.py is left)
ERASE_FILES  ?=
ERASE_FLAG   := $(if $(ERASE_FILES),--erase-parts vfs,)

# Mount the parent directory so Cargo inside Docker can resolve `micropython-rs = { path = ".." }`
WORKSPACE_ROOT := $(abspath ..)

# Persist the Cargo registry between Docker builds (no re-download of crates on every build)
CARGO_CACHE  := $(abspath target/cargo-registry)

# Extra `docker run` options. CI runs the container as root (the runner's user
# id differs from the image's `esp` user), with the toolchain paths of `esp`:
#   DOCKER_FLAGS="--user root -e RUSTUP_HOME=/home/esp/.rustup -e CARGO_HOME=/home/esp/.cargo"
DOCKER_FLAGS ?=

# Limits checked by `make report` (tools/fw_report.py): the main stack must keep
# MIN_STACK_KB, and no function may take more than MAX_FRAME_KB of stack
MIN_STACK_KB ?= 128
MAX_FRAME_KB ?= 32

.PHONY: all build flash monitor erase-files report clean

# Default target: compile inside Docker, then flash and open the serial monitor on the host
all: flash

# Compile release firmware inside Espressif's Xtensa Docker container
build:
	mkdir -p $(CARGO_CACHE)
	docker run --rm $(DOCKER_FLAGS) \
		-v "$(WORKSPACE_ROOT)":/workspace \
		-v "$(CARGO_CACHE)":/home/esp/.cargo/registry \
		-e MPY_SCRIPT="$(SCRIPT)" \
		-e MPY_CPU_MHZ="$(CPU_MHZ)" \
		-w /workspace/$(BIN_NAME) \
		$(DOCKER_IMAGE) \
		bash -c "source /home/esp/export-esp.sh && cargo build --release $(FEATURES_FLAG)"

# Build via Docker, then flash to the ESP32 and open the serial monitor
flash: build
	$(ESPFLASH) flash $(PORT_FLAG) --partition-table $(PARTITIONS) $(ERASE_FLAG) --monitor $(ELF_PATH)

# Delete all files on the board (the `vfs` partition) without reflashing the firmware
erase-files:
	$(ESPFLASH) erase-parts $(PORT_FLAG) --partition-table $(PARTITIONS) vfs

# Open the serial monitor without rebuilding or reflashing
monitor:
	$(ESPFLASH) monitor $(PORT_FLAG)

# Memory report of the last build: region sizes, main stack, biggest stack frames
report:
	python3 tools/fw_report.py $(ELF_PATH) --min-stack-kb $(MIN_STACK_KB) --max-frame-kb $(MAX_FRAME_KB)

# Remove build artifacts
clean:
	rm -rf target
