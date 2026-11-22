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

# Mount the parent directory so Cargo inside Docker can resolve `micropython-rs = { path = ".." }`
WORKSPACE_ROOT := $(abspath ..)

.PHONY: all build flash monitor clean

# Default target: compile inside Docker, then flash and open the serial monitor on the host
all: flash

# Compile release firmware inside Espressif's Xtensa Docker container
build:
	docker run --rm \
		-v "$(WORKSPACE_ROOT)":/workspace \
		-w /workspace/$(BIN_NAME) \
		$(DOCKER_IMAGE) \
		bash -c "source /home/esp/export-esp.sh && cargo build --release $(FEATURES_FLAG)"

# Build via Docker, then flash to the ESP32 and open the serial monitor
flash: build
	$(ESPFLASH) flash $(PORT_FLAG) --monitor $(ELF_PATH)

# Open the serial monitor without rebuilding or reflashing
monitor:
	$(ESPFLASH) monitor $(PORT_FLAG)

# Remove build artifacts
clean:
	rm -rf target
