.PHONY: all build build-plugin build-plugin-dev build-cli build-cli-dev dev run check clean

TARGET_WASM = wasm32-wasip1
DIST_DIR = dist
PLUGIN_WASM = $(DIST_DIR)/verij_plugin.wasm

all: dev
build: dev

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

build-plugin: $(DIST_DIR)
	cargo build -p verij-plugin --target $(TARGET_WASM) --release
	cp target/$(TARGET_WASM)/release/verij-plugin.wasm $(PLUGIN_WASM)

build-plugin-dev: $(DIST_DIR)
	cargo build -p verij-plugin --target $(TARGET_WASM)
	cp target/$(TARGET_WASM)/debug/verij-plugin.wasm $(PLUGIN_WASM)

build-cli:
	cargo build -p verij-cli --release

build-cli-dev:
	cargo build -p verij-cli

dev: build-plugin build-cli
	@echo "Build complete. Artifacts ready:"
	@echo "  CLI: target/release/verij"
	@echo "  WASM: $(PLUGIN_WASM)"

check:
	cargo check -p verij-cli
	cargo check -p verij-plugin --target $(TARGET_WASM)

run: dev
	target/release/verij start

clean:
	cargo clean
	rm -rf $(DIST_DIR)
