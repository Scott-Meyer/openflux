# Builds keyglass-bridge and its opkg package.
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

all: $(BUILD_DIR)/keyglass-bridge

$(BUILD_DIR)/keyglass-bridge: Cargo.toml build.rs src/*.rs src/bin/keyglass_bridge.rs FORCE
	@mkdir -p $(BUILD_DIR)
	KEYGLASS_VERSION=$(VERSION) RUSTFLAGS="-C target-feature=+crt-static" $(CARGO) build --release -p keyglass-bridge --target $(TARGET) --target-dir $(TARGET_DIR)
	cp $(TARGET_DIR)/$(TARGET)/release/keyglass-bridge $@

test:
	$(CARGO) test -p keyglass-bridge

ipk: $(BUILD_DIR)/keyglass-bridge
	@rm -rf $(BUILD_DIR)/_ipk_stage
	@mkdir -p $(BUILD_DIR)/_ipk_stage/control $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge $(OUT_DIR)
	@sed 's/^Version:.*/Version: $(VERSION)/' packaging/control > $(BUILD_DIR)/_ipk_stage/control/control
	@cp packaging/preinst packaging/postinst packaging/prerm $(BUILD_DIR)/_ipk_stage/control/
	@sed -i.bak 's/@KEYGLASS_VERSION@/$(VERSION)/g' $(BUILD_DIR)/_ipk_stage/control/preinst && rm -f $(BUILD_DIR)/_ipk_stage/control/preinst.bak
	@sed -i.bak 's/@KEYGLASS_VERSION@/$(VERSION)/g' $(BUILD_DIR)/_ipk_stage/control/postinst && rm -f $(BUILD_DIR)/_ipk_stage/control/postinst.bak
	@cp $(BUILD_DIR)/keyglass-bridge $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge/keyglass-bridge
	@cp packaging/keyglass-autolaunch.sh $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge/keyglass-autolaunch.sh
	@cp packaging/restore-factory.sh $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge/restore-factory.sh
	@chmod 0755 $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge/*
	$(RUST_LICENSES) --manifest-path $(CURDIR)/Cargo.toml --target $(TARGET) \
		-o $(BUILD_DIR)/_ipk_stage/data/usr/lib/keyglass/bridge/licenses/keyglass-bridge-rust-licenses.txt
	python3 $(ROOT_DIR)/scripts/build_ipk.py \
		--control-dir $(BUILD_DIR)/_ipk_stage/control \
		--data-dir $(BUILD_DIR)/_ipk_stage/data \
		-o $(OUT_DIR)/keyglass-bridge_$(VERSION)_aarch64.ipk
	@rm -rf $(BUILD_DIR)/_ipk_stage

factory-restore-ipk:
	@rm -rf $(BUILD_DIR)/_fr_stage
	@mkdir -p $(BUILD_DIR)/_fr_stage/control $(BUILD_DIR)/_fr_stage/data/home/root/.keyglass $(OUT_DIR)
	@sed 's/^Version:.*/Version: $(VERSION)/' factory-restore-packaging/control > $(BUILD_DIR)/_fr_stage/control/control
	@cp factory-restore-packaging/postinst $(BUILD_DIR)/_fr_stage/control/postinst
	@chmod 0755 $(BUILD_DIR)/_fr_stage/control/postinst
	python3 $(ROOT_DIR)/scripts/build_ipk.py \
		--control-dir $(BUILD_DIR)/_fr_stage/control \
		--data-dir $(BUILD_DIR)/_fr_stage/data \
		-o $(OUT_DIR)/keyglass-factory-restore_$(VERSION)_aarch64.ipk
	@rm -rf $(BUILD_DIR)/_fr_stage

clean:
	rm -rf $(BUILD_DIR)

FORCE:

.PHONY: all test ipk factory-restore-ipk clean FORCE
