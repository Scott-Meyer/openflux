# Builds openflux-display and its opkg package.
BUILD_DIR ?= build
CARGO ?= cargo
TARGET ?= aarch64-unknown-linux-gnu
TARGET_DIR ?= $(CURDIR)/../../target
VERSION ?= 1.0.0
OUT_DIR ?= $(BUILD_DIR)
ROOT_DIR := $(abspath $(CURDIR)/../..)
FFMPEG_DIR ?= /opt/openflux-ffmpeg
FFMPEG_BIN ?= $(FFMPEG_DIR)/openflux-ffmpeg
FFMPEG_SRC ?= $(FFMPEG_DIR)/openflux-ffmpeg-source-6.1.2.tar.xz
FFMPEG_BUILD_TXT ?= $(FFMPEG_DIR)/openflux-ffmpeg-build.txt
FFMPEG_LGPL ?= $(FFMPEG_DIR)/FFMPEG-LGPL-2.1.txt
# Writes the license texts of the Rust crates in the binary; needs cargo-about
# (in the build image, or scripts/licenses/install-cargo-about.sh).
RUST_LICENSES ?= $(ROOT_DIR)/scripts/licenses/rust-licenses.sh

all: $(BUILD_DIR)/openflux-display

$(BUILD_DIR)/openflux-display: Cargo.toml build.rs src/*.rs src/bin/openflux_display.rs FORCE
	@mkdir -p $(BUILD_DIR)
	OPENFLUX_VERSION=$(VERSION) $(CARGO) build --release -p openflux-display --target $(TARGET) --target-dir $(TARGET_DIR)
	cp $(TARGET_DIR)/$(TARGET)/release/openflux-display $@

test:
	$(CARGO) test -p openflux-display

ipk: $(BUILD_DIR)/openflux-display
	@rm -rf $(BUILD_DIR)/_ipk_stage
	@mkdir -p $(BUILD_DIR)/_ipk_stage/control \
		$(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/weather_icons \
		$(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/fonts \
		$(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/default_backgrounds \
		$(OUT_DIR)
	@sed 's/^Version:.*/Version: $(VERSION)/' packaging/control > $(BUILD_DIR)/_ipk_stage/control/control
	@cp packaging/preinst packaging/postinst packaging/prerm $(BUILD_DIR)/_ipk_stage/control/
	@sed -i.bak 's/@OPENFLUX_VERSION@/$(VERSION)/g' $(BUILD_DIR)/_ipk_stage/control/postinst && rm -f $(BUILD_DIR)/_ipk_stage/control/postinst.bak
	@cp $(BUILD_DIR)/openflux-display $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/openflux-display
	@chmod 0755 $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/openflux-display
	$(RUST_LICENSES) --manifest-path $(CURDIR)/Cargo.toml --target $(TARGET) \
		-o $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/licenses/openflux-display-rust-licenses.txt
	@if [ -f "$(FFMPEG_BIN)" ] || [ -f "$(BUILD_DIR)/openflux-ffmpeg" ]; then \
		FF_SEL="$(FFMPEG_BIN)"; \
		if [ ! -f "$$FF_SEL" ]; then FF_SEL="$(BUILD_DIR)/openflux-ffmpeg"; fi; \
		test -f "$(FFMPEG_LGPL)" && test -f "$(FFMPEG_SRC)" && test -f "$(FFMPEG_BUILD_TXT)" || { \
			echo "ERROR: openflux-ffmpeg requires FFMPEG-LGPL-2.1.txt, source tarball, and build metadata in $(FFMPEG_DIR)" >&2; \
			exit 1; \
		}; \
		cp "$$FF_SEL" $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/openflux-ffmpeg && \
		chmod 0755 $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/openflux-ffmpeg && \
		mkdir -p $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/licenses && \
		cp "$(FFMPEG_LGPL)" $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/licenses/FFMPEG-LGPL-2.1.txt && \
		printf '%s\n' \
			'openflux-ffmpeg uses FFmpeg 6.1.2 (LGPL-2.1-or-later).' \
			'SHA-256: 3b624649725ecdc565c903ca6643d41f33bd49239922e45c9b1442c63dca4e38' \
			'The corresponding source archive (openflux-ffmpeg-source-6.1.2.tar.xz) and build configuration (openflux-ffmpeg-build.txt) are published alongside each OpenFlux release.' \
			> $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/licenses/FFMPEG-NOTICE.txt && \
		cp "$(FFMPEG_SRC)" "$(OUT_DIR)/openflux-ffmpeg-source-6.1.2.tar.xz" && \
		cp "$(FFMPEG_BUILD_TXT)" "$(OUT_DIR)/openflux-ffmpeg-build.txt"; \
	fi
	@cp $(ROOT_DIR)/assets/weather_icons/* $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/weather_icons/
	@cp $(ROOT_DIR)/assets/bundled_fonts/* $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/fonts/
	@cp -R $(ROOT_DIR)/assets/default_backgrounds/. $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/default_backgrounds/
	@cp $(ROOT_DIR)/assets/LICENSE_MANIFEST.md $(ROOT_DIR)/assets/LUCIDE_LICENSE $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/display/
	python3 $(ROOT_DIR)/scripts/build_ipk.py \
		--control-dir $(BUILD_DIR)/_ipk_stage/control \
		--data-dir $(BUILD_DIR)/_ipk_stage/data \
		-o $(OUT_DIR)/openflux-display_$(VERSION)_aarch64.ipk
	@rm -rf $(BUILD_DIR)/_ipk_stage

clean:
	rm -rf $(BUILD_DIR)

FORCE:

.PHONY: all test ipk clean FORCE
