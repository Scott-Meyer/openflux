# macOS helper executables shipped inside Keyglass Studio.app.
SWIFTC ?= swiftc
BUILD_DIR ?= build

all: $(BUILD_DIR)/keyglass-sensors $(BUILD_DIR)/keyglass-tray

$(BUILD_DIR)/keyglass-sensors: keyglass_sensors.swift
	@mkdir -p $(BUILD_DIR)
	$(SWIFTC) -O -framework AppKit -framework Carbon -framework IOKit -o $@ $<

$(BUILD_DIR)/keyglass-tray: keyglass_tray.swift
	@mkdir -p $(BUILD_DIR)
	$(SWIFTC) -O -framework AppKit -framework Security -o $@ $<

# Runs the sensor helper's self-test.
check: $(BUILD_DIR)/keyglass-sensors
	$(BUILD_DIR)/keyglass-sensors --test-late-callback

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all check clean
