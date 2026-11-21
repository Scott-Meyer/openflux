# Builds openflux-bridge and its opkg package.
BUILD_DIR ?= build
CARGO ?= cargo
TARGET ?= aarch64-unknown-linux-musl
TARGET_DIR ?= $(CURDIR)/../../target
VERSION ?= 1.0.0
OUT_DIR ?= $(BUILD_DIR)
ROOT_DIR := $(abspath $(CURDIR)/../..)
# Writes the license texts of the Rust crates in the binary; needs cargo-about
# (in the build image, or scripts/licenses/install-cargo-about.sh).
RUST_LICENSES ?= $(ROOT_DIR)/scripts/licenses/rust-licenses.sh

all: $(BUILD_DIR)/openflux-bridge

$(BUILD_DIR)/openflux-bridge: Cargo.toml build.rs src/*.rs src/bin/openflux_bridge.rs FORCE
	@mkdir -p $(BUILD_DIR)
	OPENFLUX_VERSION=$(VERSION) RUSTFLAGS="-C target-feature=+crt-static" $(CARGO) build --release -p openflux-bridge --target $(TARGET) --target-dir $(TARGET_DIR)
	cp $(TARGET_DIR)/$(TARGET)/release/openflux-bridge $@

test:
	$(CARGO) test -p openflux-bridge

ipk: $(BUILD_DIR)/openflux-bridge
	@rm -rf $(BUILD_DIR)/_ipk_stage
	@mkdir -p $(BUILD_DIR)/_ipk_stage/control $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge $(OUT_DIR)
	@sed 's/^Version:.*/Version: $(VERSION)/' packaging/control > $(BUILD_DIR)/_ipk_stage/control/control
	@cp packaging/preinst packaging/postinst packaging/prerm $(BUILD_DIR)/_ipk_stage/control/
	@sed -i.bak 's/@OPENFLUX_VERSION@/$(VERSION)/g' $(BUILD_DIR)/_ipk_stage/control/preinst && rm -f $(BUILD_DIR)/_ipk_stage/control/preinst.bak
	@sed -i.bak 's/@OPENFLUX_VERSION@/$(VERSION)/g' $(BUILD_DIR)/_ipk_stage/control/postinst && rm -f $(BUILD_DIR)/_ipk_stage/control/postinst.bak
	@cp $(BUILD_DIR)/openflux-bridge $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge/openflux-bridge
	@cp packaging/openflux-autolaunch.sh $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge/openflux-autolaunch.sh
	@cp packaging/restore-factory.sh $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge/restore-factory.sh
	@chmod 0755 $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge/*
	$(RUST_LICENSES) --manifest-path $(CURDIR)/Cargo.toml --target $(TARGET) \
		-o $(BUILD_DIR)/_ipk_stage/data/usr/lib/openflux/bridge/licenses/openflux-bridge-rust-licenses.txt
	python3 $(ROOT_DIR)/scripts/build_ipk.py \
		--control-dir $(BUILD_DIR)/_ipk_stage/control \
		--data-dir $(BUILD_DIR)/_ipk_stage/data \
		-o $(OUT_DIR)/openflux-bridge_$(VERSION)_aarch64.ipk
	@rm -rf $(BUILD_DIR)/_ipk_stage

factory-restore-ipk:
	@rm -rf $(BUILD_DIR)/_fr_stage
	@mkdir -p $(BUILD_DIR)/_fr_stage/control $(BUILD_DIR)/_fr_stage/data/home/root/.openflux $(OUT_DIR)
	@sed 's/^Version:.*/Version: $(VERSION)/' factory-restore-packaging/control > $(BUILD_DIR)/_fr_stage/control/control
	@cp factory-restore-packaging/postinst $(BUILD_DIR)/_fr_stage/control/postinst
	@chmod 0755 $(BUILD_DIR)/_fr_stage/control/postinst
	python3 $(ROOT_DIR)/scripts/build_ipk.py \
		--control-dir $(BUILD_DIR)/_fr_stage/control \
		--data-dir $(BUILD_DIR)/_fr_stage/data \
		-o $(OUT_DIR)/openflux-factory-restore_$(VERSION)_aarch64.ipk
	@rm -rf $(BUILD_DIR)/_fr_stage

clean:
	rm -rf $(BUILD_DIR)

FORCE:

.PHONY: all test ipk factory-restore-ipk clean FORCE
