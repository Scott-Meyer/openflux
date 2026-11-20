# macOS helper executables shipped inside OpenFlux Studio.app.
SWIFTC ?= swiftc
BUILD_DIR ?= build

all: $(BUILD_DIR)/openflux-sensors $(BUILD_DIR)/openflux-tray

$(BUILD_DIR)/openflux-sensors: openflux_sensors.swift
	@mkdir -p $(BUILD_DIR)
	$(SWIFTC) -O -framework AppKit -framework Carbon -framework IOKit -o $@ $<

$(BUILD_DIR)/openflux-tray: openflux_tray.swift
	@mkdir -p $(BUILD_DIR)
	$(SWIFTC) -O -framework AppKit -framework Security -o $@ $<

# Runs the sensor helper's self-test.
check: $(BUILD_DIR)/openflux-sensors
	$(BUILD_DIR)/openflux-sensors --test-late-callback

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all check clean
